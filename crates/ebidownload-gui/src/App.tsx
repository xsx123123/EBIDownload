import { useState, useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import './styles.css';

// Types
interface EnaRecord {
  run_accession: string;
  sample_title: string;
  library_layout: string | null;
}

interface Config {
  software: {
    prefetch: string;
    fasterq_dump: string;
  };
}

interface DownloadEvent {
  type: 'Log' | 'Progress' | 'Started' | 'Completed' | 'Error' | 'DryRun' | 'Metadata';
  data: any;
}

interface UploadEvent {
  type: 'Log' | 'Progress' | 'Started' | 'Completed' | 'Error' | 'DryRun';
  data: any;
}

interface LogEntry {
  level: string;
  message: string;
}

interface DepStatusReady {
  status: 'Ready';
  prefetch: string;
  fasterq_dump: string;
  source: string;
}

interface DepStatusMissing {
  status: 'Missing';
  reason: string;
}

type DepStatus = DepStatusReady | DepStatusMissing;

interface DepInstallProgress {
  step: string;
  percent: number;
  message: string;
}

interface TransferProgress {
  percent: number;
  status: string;
  speed_mbps: number;
}

interface GlobalProgress {
  total: number;
  completed: number;
  active: number;
  percent: number;
  speed_mbps: number;
}

function App() {
  const [activeTab, setActiveTab] = useState<'download' | 'upload' | 'settings' | 'about'>('download');
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    if (typeof window === 'undefined') return 'dark';
    const saved = window.localStorage.getItem('ebidownload-theme');
    if (saved === 'light' || saved === 'dark') return saved;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  });
  const [_config, setConfig] = useState<Config | null>(null);
  const [metadata, setMetadata] = useState<EnaRecord[]>([]);
  const [isDownloading, setIsDownloading] = useState(false);
  const [isUploading, setIsUploading] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<Record<string, TransferProgress>>({});
  const [globalProgress, setGlobalProgress] = useState<GlobalProgress | null>(null);
  const [uploadProgress, setUploadProgress] = useState<Record<string, { percent: number; status: string }>>({});
  const [logs, setLogs] = useState<{ level: string; message: string; time: string }[]>([]);
  const [logLevelFilter, setLogLevelFilter] = useState<string>('info');
  const [metadataExpanded, setMetadataExpanded] = useState(true);
  const [downloadConfigCollapsed, setDownloadConfigCollapsed] = useState(false);
  const [depStatus, setDepStatus] = useState<'checking' | 'ready' | 'missing'>('checking');
  const [depInfo, setDepInfo] = useState<DepStatus | null>(null);
  const [isInstallingDeps, setIsInstallingDeps] = useState(false);
  const [depProgress, setDepProgress] = useState<DepInstallProgress | null>(null);
  const [depModalDismissed, setDepModalDismissed] = useState(false);

  // Download form state
  const [downloadForm, setDownloadForm] = useState({
    accession: '',
    tsv: '',
    output: '',
    method: 'Aws' as 'Ftp' | 'Prefetch' | 'Aws' | 'Auto',
    multithreads: 4,
    awsThreads: 8,
    chunkSize: 20,
    peOnly: false,
    cleanupSra: false,
    dryRun: false,
    filterSample: '',
    filterRun: '',
    excludeSample: '',
    excludeRun: '',
  });

  // Upload form state
  const [uploadForm, setUploadForm] = useState({
    bucket: '',
    prefix: '',
    files: [] as string[],
    region: 'us-east-1',
    concurrent: 4,
    applyPolicy: false,
    metadataTemplate: '',
    dryRun: false,
  });

  // Config form state
  const [configForm, setConfigForm] = useState({
    prefetchPath: '',
    fasterqDumpPath: '',
  });
  const [configPath, setConfigPath] = useState('');

  // Apply theme to <html> and persist to localStorage
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    try {
      window.localStorage.setItem('ebidownload-theme', theme);
    } catch (_) {
      // ignore storage errors (e.g. private mode)
    }
  }, [theme]);

  const toggleTheme = () => {
    setTheme(prev => (prev === 'dark' ? 'light' : 'dark'));
  };

  // Load config and check dependencies on startup
  useEffect(() => {
    const initialize = async () => {
      await loadConfigPath();
      await loadConfig();
      await checkDeps();
    };
    initialize();
  }, []);

  // Setup event listeners
  useEffect(() => {
    const unlistenDownload = listen<DownloadEvent>('download-event', (event) => {
      handleDownloadEvent(event.payload);
    });

    const unlistenUpload = listen<UploadEvent>('upload-event', (event) => {
      handleUploadEvent(event.payload);
    });

    const unlistenLog = listen<LogEntry>('app-log', (event) => {
      addLog(event.payload.level, event.payload.message);
    });

    const unlistenDepsInstalled = listen('deps-installed', () => {
      addLog('info', 'Dependencies installed');
      setIsInstallingDeps(false);
      setDepProgress(null);
      setDepModalDismissed(false);
      checkDeps();
    });

    const unlistenDepProgress = listen<DepInstallProgress>('dep-progress', (event) => {
      setDepProgress(event.payload);
    });

    return () => {
      unlistenDownload.then(f => f());
      unlistenUpload.then(f => f());
      unlistenLog.then(f => f());
      unlistenDepsInstalled.then(f => f());
      unlistenDepProgress.then(f => f());
    };
  }, []);

  const checkDeps = async () => {
    try {
      const status = await invoke<DepStatus>('check_deps_command');
      setDepInfo(status);
      if (status.status === 'Ready') {
        setDepStatus('ready');
        addLog('info', `sra-tools ready (${status.source}): prefetch=${status.prefetch}, fasterq-dump=${status.fasterq_dump}`);
      } else {
        setDepStatus('missing');
        addLog('warn', `sra-tools missing: ${status.reason}`);
      }
    } catch (e) {
      setDepStatus('missing');
      addLog('error', `Failed to check dependencies: ${e}`);
    }
  };

  const handleInstallDeps = async () => {
    if (isInstallingDeps) return;
    setIsInstallingDeps(true);
    setDepProgress({
      step: 'prepare',
      percent: 0,
      message: 'Preparing to install sra-tools...',
    });
    addLog('info', 'Installing sra-tools...');
    try {
      await invoke('install_deps_command');
    } catch (e) {
      addLog('error', `Failed to install sra-tools: ${e}`);
      setIsInstallingDeps(false);
      setDepProgress(null);
    }
  };

  const loadConfigPath = async () => {
    try {
      const path = await invoke<string>('get_config_path_command');
      setConfigPath(path);
    } catch (e) {
      addLog('warn', `Failed to get config path: ${e}`);
    }
  };

  const loadConfig = async () => {
    try {
      const result = await invoke<Config | null>('get_config_command');
      if (result) {
        setConfig(result);
        setConfigForm({
          prefetchPath: result.software.prefetch,
          fasterqDumpPath: result.software.fasterq_dump,
        });
      }
    } catch (e) {
      addLog('warn', `Failed to load config: ${e}`);
    }
  };

  const handleSelectConfigPath = async () => {
    const selected = await save({
      filters: [{ name: 'YAML', extensions: ['yaml', 'yml'] }],
      defaultPath: 'EBIDownload.yaml',
    });
    if (selected) {
      try {
        await invoke('set_config_path_command', { path: selected });
        setConfigPath(selected);
        addLog('info', `Config path set to: ${selected}`);
        await loadConfig();
        await checkDeps();
      } catch (e) {
        addLog('error', `Failed to set config path: ${e}`);
      }
    }
  };

  const addLog = (level: string, message: string) => {
    const time = new Date().toLocaleTimeString([], { hour12: false });
    setLogs(prev => [...prev.slice(-100), { level, message, time }]);
  };

  const handleDownloadEvent = (event: DownloadEvent) => {
    switch (event.type) {
      case 'Log':
        addLog(event.data.level, event.data.message);
        break;
      case 'Progress':
        setDownloadProgress(prev => {
          const next: Record<string, TransferProgress> = {
            ...prev,
            [event.data.run_id]: {
              percent: event.data.percent,
              status: event.data.status,
              speed_mbps: event.data.speed_mbps ?? 0,
            },
          };
          setGlobalProgress(current => {
            const entries = Object.values(next);
            const total = Math.max(current?.total ?? 0, entries.length);
            const completed = entries.filter(p => p.status === 'Completed').length;
            const active = entries.filter(p => p.status !== 'Completed' && p.status !== 'Failed').length;
            const percent = total > 0
              ? entries.reduce((sum, p) => sum + Math.max(0, Math.min(100, p.percent)), 0) / total
              : 0;
            const speed_mbps = entries.reduce((sum, p) => sum + p.speed_mbps, 0);
            return { total, completed, active, percent, speed_mbps };
          });
          return next;
        });
        break;
      case 'Started':
        setIsDownloading(true);
        setDownloadProgress({});
        setGlobalProgress({ total: event.data.total, completed: 0, active: 0, percent: 0, speed_mbps: 0 });
        addLog('info', `Starting download of ${event.data.total} files`);
        break;
      case 'Completed':
        setIsDownloading(false);
        setIsPaused(false);
        setDownloadConfigCollapsed(false);
        addLog('info', 'Download completed successfully');
        break;
      case 'Error':
        setIsDownloading(false);
        setIsPaused(false);
        setDownloadConfigCollapsed(false);
        addLog('error', event.data.message);
        break;
      case 'Metadata':
        setMetadata(event.data.records);
        break;
      case 'DryRun':
        addLog('info', `Dry run: would download ${event.data.files.length} files`);
        break;
    }
  };

  const handleUploadEvent = (event: UploadEvent) => {
    switch (event.type) {
      case 'Log':
        addLog(event.data.level, event.data.message);
        break;
      case 'Progress':
        setUploadProgress(prev => ({
          ...prev,
          [event.data.filename]: {
            percent: event.data.percent,
            status: event.data.status,
          },
        }));
        break;
      case 'Started':
        setIsUploading(true);
        addLog('info', `Starting upload of ${event.data.total} files`);
        break;
      case 'Completed':
        setIsUploading(false);
        addLog('info', 'Upload completed successfully');
        break;
      case 'Error':
        setIsUploading(false);
        addLog('error', event.data.message);
        break;
      case 'DryRun':
        addLog('info', `Dry run: would upload ${event.data.files.length} files`);
        break;
    }
  };

  const handleFetchMetadata = async () => {
    try {
      const accession = downloadForm.accession || undefined;
      const tsv = downloadForm.tsv || undefined;
      const result = await invoke<EnaRecord[]>('fetch_metadata_command', { accession, tsv });
      setMetadata(result);
      addLog('info', `Fetched ${result.length} records`);
    } catch (e) {
      addLog('error', `Failed to fetch metadata: ${e}`);
    }
  };

  const handlePauseDownload = async () => {
    try {
      const nextPaused = !isPaused;
      await invoke('pause_download_command', { paused: nextPaused });
      setIsPaused(nextPaused);
      addLog('info', nextPaused ? 'Download paused' : 'Download resumed');
    } catch (e) {
      addLog('error', `Failed to pause/resume download: ${e}`);
    }
  };

  const handleCancelDownload = async () => {
    try {
      await invoke('cancel_download_command');
      setIsPaused(false);
      addLog('info', 'Download cancelled');
    } catch (e) {
      addLog('error', `Failed to cancel download: ${e}`);
    }
  };

  const handleStartDownload = async () => {
    if (!downloadForm.output) {
      addLog('error', 'Please select an output directory');
      return;
    }

    // Reset pause state on a fresh download.
    setIsPaused(false);

    // Collapse configuration cards and expand the progress panel.
    setDownloadConfigCollapsed(true);
    setMetadataExpanded(true);

    try {
      const splitFilters = (s: string) => s.split(/[,\n]/).map(item => item.trim()).filter(item => item !== '');
      
      await invoke('start_download_command', {
        options: {
          accession: downloadForm.accession || null,
          tsv: downloadForm.tsv || null,
          output: downloadForm.output,
          downloadMethod: downloadForm.method,
          multithreads: downloadForm.multithreads,
          awsThreads: downloadForm.awsThreads,
          chunkSize: downloadForm.chunkSize,
          prefetchMaxSize: '100G',
          peOnly: downloadForm.peOnly,
          filterSample: splitFilters(downloadForm.filterSample),
          filterRun: splitFilters(downloadForm.filterRun),
          excludeSample: splitFilters(downloadForm.excludeSample),
          excludeRun: splitFilters(downloadForm.excludeRun),
          cleanupSra: downloadForm.cleanupSra,
          dryRun: downloadForm.dryRun,
        },
      });
    } catch (e) {
      addLog('error', `Failed to start download: ${e}`);
    }
  };

  const handleStartUpload = async () => {
    if (!uploadForm.bucket) {
      addLog('error', 'Please enter a bucket name');
      return;
    }
    if (uploadForm.files.length === 0) {
      addLog('error', 'Please select files to upload');
      return;
    }

    try {
      await invoke('start_upload_command', {
        options: {
          bucket: uploadForm.bucket,
          prefix: uploadForm.prefix || null,
          files: uploadForm.files,
          region: uploadForm.region,
          concurrent: uploadForm.concurrent,
          applyPolicy: uploadForm.applyPolicy,
          metadataTemplate: uploadForm.metadataTemplate || null,
          dryRun: uploadForm.dryRun,
        },
      });
    } catch (e) {
      addLog('error', `Failed to start upload: ${e}`);
    }
  };

  const handleSaveConfig = async () => {
    try {
      await invoke('save_config_command', {
        config: {
          prefetchPath: configForm.prefetchPath,
          fasterqDumpPath: configForm.fasterqDumpPath,
        },
      });
      addLog('info', 'Config saved successfully');
      await loadConfig();
      // Re-check dependencies after manual path configuration.
      await checkDeps();
    } catch (e) {
      addLog('error', `Failed to save config: ${e}`);
    }
  };

  const selectFile = async (multiple = false, filter?: string) => {
    const selected = await open({
      multiple,
      filters: filter ? [{ name: 'Files', extensions: [filter] }] : undefined,
    });
    return selected;
  };

  const selectDirectory = async () => {
    const selected = await open({
      directory: true,
    });
    return selected;
  };

  return (
    <div className="container">
      {depStatus === 'missing' && !depModalDismissed && (
        <div className="modal-overlay">
          <div className="modal-card dep-modal">
            <button
              className="modal-close"
              onClick={() => setDepModalDismissed(true)}
              aria-label="Close"
              disabled={isInstallingDeps}
            >
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>

            <div className="modal-icon">
              <svg viewBox="0 0 24 24" width="40" height="40" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                <polyline points="17 8 12 3 7 8"></polyline>
                <line x1="12" y1="3" x2="12" y2="15"></line>
              </svg>
            </div>

            <h2>sra-tools not found</h2>
            <p className="modal-subtitle">
              EBIDownload needs NCBI <code>sra-tools</code> (<code>prefetch</code> & <code>fasterq-dump</code>) to download SRA data.
            </p>

            {depInfo?.status === 'Missing' && (
              <div className="modal-reason">{depInfo.reason}</div>
            )}

            {isInstallingDeps && depProgress && (
              <div className="dep-progress">
                <div className="dep-progress-header">
                  <span>{depProgress.message}</span>
                  <span>{Math.round(depProgress.percent)}%</span>
                </div>
                <div className="progress-bar-container" style={{ height: '8px' }}>
                  <div
                    className="progress-bar"
                    style={{
                      width: `${Math.min(100, Math.max(0, depProgress.percent))}%`,
                      transition: 'width 0.3s ease',
                    }}
                  />
                </div>
              </div>
            )}

            <div className="modal-actions">
              <button
                className="btn btn-primary btn-install"
                onClick={handleInstallDeps}
                disabled={isInstallingDeps}
              >
                {isInstallingDeps ? (
                  <>
                    <svg className="animate-spin" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3">
                      <path d="M21 12a9 9 0 1 1-6.219-8.56"></path>
                    </svg>
                    Installing...
                  </>
                ) : (
                  <>
                    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                      <polyline points="7 10 12 15 17 10"></polyline>
                      <line x1="12" y1="15" x2="12" y2="3"></line>
                    </svg>
                    Auto-install sra-tools
                  </>
                )}
              </button>
              <button
                className="btn btn-secondary btn-configure"
                onClick={() => {
                  setDepModalDismissed(true);
                  setActiveTab('settings');
                }}
                disabled={isInstallingDeps}
              >
                Configure manually
              </button>
            </div>
          </div>
        </div>
      )}

      {depStatus === 'missing' && depModalDismissed && (
        <div className="dep-warning-banner">
          <span>sra-tools is missing. Some download features may not work.</span>
          <div className="dep-warning-actions">
            <button className="btn-link" onClick={() => setDepModalDismissed(false)}>
              Install now
            </button>
            <button className="btn-link" onClick={() => setActiveTab('settings')}>
              Configure paths
            </button>
          </div>
        </div>
      )}

      <div className="header">
        <button
          type="button"
          className="theme-toggle"
          onClick={toggleTheme}
          aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
          title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
        >
          {theme === 'dark' ? (
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="4"></circle>
              <path d="M12 2v2"></path>
              <path d="M12 20v2"></path>
              <path d="m4.93 4.93 1.41 1.41"></path>
              <path d="m17.66 17.66 1.41 1.41"></path>
              <path d="M2 12h2"></path>
              <path d="M20 12h2"></path>
              <path d="m6.34 17.66-1.41 1.41"></path>
              <path d="m19.07 4.93-1.41 1.41"></path>
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
            </svg>
          )}
        </button>
        <h1>EBIDownload</h1>
        <p style={{ color: 'var(--text-muted)', marginTop: '0.25rem' }}>Automated Bioinformatics Data Retrieval</p>
      </div>

      <div className="tabs">
        <button
          className={`tab ${activeTab === 'download' ? 'active' : ''}`}
          onClick={() => setActiveTab('download')}
        >
          Download
        </button>
        <button
          className={`tab ${activeTab === 'upload' ? 'active' : ''}`}
          onClick={() => setActiveTab('upload')}
        >
          Upload
        </button>
        <button
          className={`tab ${activeTab === 'settings' ? 'active' : ''}`}
          onClick={() => setActiveTab('settings')}
        >
          Settings
        </button>
        <button
          className={`tab ${activeTab === 'about' ? 'active' : ''}`}
          onClick={() => setActiveTab('about')}
        >
          About
        </button>
      </div>

      <div className="tab-content">
        {activeTab === 'download' && (
          <DownloadTab
            form={downloadForm}
            setForm={setDownloadForm}
            metadata={metadata}
            progress={downloadProgress}
            globalProgress={globalProgress}
            isDownloading={isDownloading}
            isPaused={isPaused}
            configCollapsed={downloadConfigCollapsed}
            setConfigCollapsed={setDownloadConfigCollapsed}
            onFetchMetadata={handleFetchMetadata}
            onStartDownload={handleStartDownload}
            onPauseDownload={handlePauseDownload}
            onCancelDownload={handleCancelDownload}
            selectFile={selectFile}
            selectDirectory={selectDirectory}
            metadataExpanded={metadataExpanded}
            setMetadataExpanded={setMetadataExpanded}
          />
        )}

        {activeTab === 'upload' && (
          <UploadTab
            form={uploadForm}
            setForm={setUploadForm}
            progress={uploadProgress}
            isUploading={isUploading}
            onStartUpload={handleStartUpload}
            selectFile={selectFile}
          />
        )}

        {activeTab === 'settings' && (
          <SettingsTab
            form={configForm}
            setForm={setConfigForm}
            onSaveConfig={handleSaveConfig}
            selectFile={selectFile}
            configPath={configPath}
            onSelectConfigPath={handleSelectConfigPath}
          />
        )}

        {activeTab === 'about' && <AboutTab />}
      </div>

      <div className="card" style={{ marginTop: '2rem' }}>
        <div className="card-title" style={{ justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
            </svg>
            Logs
          </div>
          <select
            className="log-filter"
            value={logLevelFilter}
            onChange={(e) => setLogLevelFilter(e.target.value)}
          >
            <option value="all">All levels</option>
            <option value="info">INFO</option>
            <option value="warn">WARN</option>
            <option value="error">ERROR</option>
          </select>
        </div>
        <div className="log-container">
          {logs.filter(log => logLevelFilter === 'all' || log.level === logLevelFilter).length === 0 ? (
            <div style={{ color: 'var(--text-muted)', fontStyle: 'italic' }}>
              {logLevelFilter === 'all' ? 'System logs will appear here...' : `No ${logLevelFilter.toUpperCase()} logs to display.`}
            </div>
          ) : (
            logs
              .filter(log => logLevelFilter === 'all' || log.level === logLevelFilter)
              .map((log, i) => (
                <div key={i} className={`log-entry log-${log.level}`}>
                  <span className="log-time">[{log.time}]</span>
                  <span className="log-level">{log.level.toUpperCase()}</span>
                  <span className="log-msg">{log.message}</span>
                </div>
              ))
          )}
        </div>
      </div>
    </div>
  );
}

// Helper for status badge class
const getStatusClass = (status: string) => {
  if (status === 'Downloading') return 'status-downloading';
  if (status === 'Converting' || status === 'Compressing') return 'status-converting';
  if (status === 'Completed') return 'status-completed';
  if (status === 'Failed') return 'status-failed';
  return '';
};

// Helper for progress bar class
const getProgressBarClass = (status: string) => {
  if (status === 'Downloading') return 'active';
  if (status === 'Converting' || status === 'Compressing') return 'converting';
  if (status === 'Completed') return 'completed';
  if (status === 'Failed') return 'failed';
  return '';
};

const getStageIndex = (status: string) => {
  if (status === 'Completed') return 3;
  if (status === 'Compressing') return 2;
  if (status === 'Converting') return 1;
  return 0;
};

const clampPercent = (percent: number) => Math.max(0, Math.min(100, percent));

// Download Tab Component
function DownloadTab({
  form,
  setForm,
  metadata,
  progress,
  globalProgress,
  isDownloading,
  isPaused,
  configCollapsed,
  setConfigCollapsed,
  onFetchMetadata,
  onStartDownload,
  onPauseDownload,
  onCancelDownload,
  selectFile,
  selectDirectory,
  metadataExpanded,
  setMetadataExpanded,
}: any) {
  const handleSelectTsv = async () => {
    const selected = await selectFile(false, 'tsv');
    if (selected && !Array.isArray(selected)) {
      setForm((prev: any) => ({ ...prev, tsv: selected }));
    }
  };

  const handleSelectOutput = async () => {
    const selected = await selectDirectory();
    if (selected && !Array.isArray(selected)) {
      setForm((prev: any) => ({ ...prev, output: selected }));
    }
  };

  const shouldInclude = (record: EnaRecord) => {
    try {
      const splitFilters = (s: string) => s.split(/[,\n]/).map(item => item.trim()).filter(item => item !== '');
      
      const incSample = splitFilters(form.filterSample).map(p => new RegExp(p, 'i'));
      const incRun = splitFilters(form.filterRun).map(p => new RegExp(p, 'i'));
      const excSample = splitFilters(form.excludeSample).map(p => new RegExp(p, 'i'));
      const excRun = splitFilters(form.excludeRun).map(p => new RegExp(p, 'i'));

      if (incSample.length > 0 && !incSample.some(r => r.test(record.sample_title))) return false;
      if (incRun.length > 0 && !incRun.some(r => r.test(record.run_accession))) return false;
      if (excSample.length > 0 && excSample.some(r => r.test(record.sample_title))) return false;
      if (excRun.length > 0 && excRun.some(r => r.test(record.run_accession))) return false;
      
      return true;
    } catch (e) {
      return true;
    }
  };

  const filteredCount = metadata.filter(shouldInclude).length;

  return (
    <div className="download-tab">
      {!configCollapsed && (
        <>
      <div className="grid-2">
        <div className="card">
          <div className="card-title">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
              <polyline points="7 10 12 15 17 10"></polyline>
              <line x1="12" y1="15" x2="12" y2="3"></line>
            </svg>
            Source Information
          </div>
          <div className="form-group">
            <label>Accession (e.g., PRJNA123456)</label>
            <input
              type="text"
              value={form.accession}
              onChange={(e) => setForm((prev: any) => ({ ...prev, accession: e.target.value }))}
              disabled={isDownloading}
              placeholder="Enter Project or Study Accession"
            />
          </div>
          <div className="form-group">
            <label>Or TSV File</label>
            <div className="form-row">
              <input
                type="text"
                value={form.tsv}
                readOnly
                placeholder="Select a TSV file"
                style={{ flex: 1 }}
              />
              <button className="btn btn-secondary" onClick={handleSelectTsv} disabled={isDownloading}>
                Browse
              </button>
            </div>
          </div>
        </div>

        <div className="card">
          <div className="card-title">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
            Download Settings
          </div>
          <div className="form-group">
            <label>Output Directory</label>
            <div className="form-row">
              <input
                type="text"
                value={form.output}
                readOnly
                placeholder="Where to save files"
                style={{ flex: 1 }}
              />
              <button className="btn btn-secondary" onClick={handleSelectOutput} disabled={isDownloading}>
                Browse
              </button>
            </div>
          </div>
          <div className="form-group">
            <label>Method</label>
            <select
              value={form.method}
              onChange={(e) => setForm((prev: any) => ({ ...prev, method: e.target.value as any }))}
              disabled={isDownloading}
            >
              <option value="Aws">AWS S3 (Recommended)</option>
              <option value="Ftp">FTP</option>
              <option value="Prefetch">NCBI Prefetch</option>
              <option value="Auto">Automatic Fallback</option>
            </select>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"></polygon>
          </svg>
          Data Filtering (Regex)
        </div>
        <div className="grid-2">
          <div>
            <div className="form-group">
              <label>Include Sample (Sample Title)</label>
              <input
                type="text"
                value={form.filterSample}
                onChange={(e) => setForm((prev: any) => ({ ...prev, filterSample: e.target.value }))}
                disabled={isDownloading}
                placeholder="Regex (e.g., Liver|Lung)"
              />
            </div>
            <div className="form-group">
              <label>Include Run (Run Accession)</label>
              <input
                type="text"
                value={form.filterRun}
                onChange={(e) => setForm((prev: any) => ({ ...prev, filterRun: e.target.value }))}
                disabled={isDownloading}
                placeholder="Regex (e.g., SRR123.*)"
              />
            </div>
          </div>
          <div>
            <div className="form-group">
              <label>Exclude Sample</label>
              <input
                type="text"
                value={form.excludeSample}
                onChange={(e) => setForm((prev: any) => ({ ...prev, excludeSample: e.target.value }))}
                disabled={isDownloading}
                placeholder="Regex pattern"
              />
            </div>
            <div className="form-group">
              <label>Exclude Run</label>
              <input
                type="text"
                value={form.excludeRun}
                onChange={(e) => setForm((prev: any) => ({ ...prev, excludeRun: e.target.value }))}
                disabled={isDownloading}
                placeholder="Regex pattern"
              />
            </div>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M12 20v-6M9 20v-10M15 20v-4M18 20v-8M6 20v-2"></path>
          </svg>
          Advanced Options
        </div>
        <div className="grid-2">
          <div>
            <div className="form-group">
              <label>Parallel File Downloads</label>
              <input
                type="number"
                min={1}
                value={form.multithreads}
                onChange={(e) => setForm((prev: any) => ({ ...prev, multithreads: parseInt(e.target.value) }))}
                disabled={isDownloading}
              />
            </div>
            <div className="form-group">
              <label>Threads per File (AWS)</label>
              <input
                type="number"
                min={1}
                value={form.awsThreads}
                onChange={(e) => setForm((prev: any) => ({ ...prev, awsThreads: parseInt(e.target.value) }))}
                disabled={isDownloading}
              />
            </div>
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem', justifyContent: 'center' }}>
            <label className="checkbox-group">
              <input
                type="checkbox"
                checked={form.peOnly}
                onChange={(e) => setForm((prev: any) => ({ ...prev, peOnly: e.target.checked }))}
                disabled={isDownloading}
              />
              <span>Paired-End Only (skip Single-End)</span>
            </label>
            <label className="checkbox-group">
              <input
                type="checkbox"
                checked={form.cleanupSra}
                onChange={(e) => setForm((prev: any) => ({ ...prev, cleanupSra: e.target.checked }))}
                disabled={isDownloading}
              />
              <span>Cleanup SRA files after conversion</span>
            </label>
            <label className="checkbox-group">
              <input
                type="checkbox"
                checked={form.dryRun}
                onChange={(e) => setForm((prev: any) => ({ ...prev, dryRun: e.target.checked }))}
                disabled={isDownloading}
              />
              <span>Dry Run (simulate only)</span>
            </label>
          </div>
        </div>
      </div>
        </>
      )}

      <div style={{ display: 'flex', gap: '1rem', justifyContent: 'flex-end', marginBottom: '2rem' }}>
        {configCollapsed && !isDownloading && (
          <button
            className="btn btn-secondary"
            onClick={() => setConfigCollapsed(false)}
          >
            Show settings
          </button>
        )}
        {!configCollapsed && isDownloading && (
          <button
            className="btn btn-secondary"
            onClick={() => setConfigCollapsed(true)}
          >
            Hide settings
          </button>
        )}
        <button
          className="btn btn-secondary"
          onClick={onFetchMetadata}
          disabled={isDownloading || (!form.accession && !form.tsv)}
        >
          Check Records
        </button>
        {isDownloading && (
          <>
            <button
              className="btn btn-warning"
              onClick={onPauseDownload}
            >
              {isPaused ? 'Resume' : 'Pause'}
            </button>
            <button
              className="btn btn-danger"
              onClick={onCancelDownload}
            >
              Stop
            </button>
          </>
        )}
        <button
          className="btn btn-primary"
          onClick={onStartDownload}
          disabled={isDownloading || !form.output || (!form.accession && !form.tsv)}
        >
          {isDownloading ? (
            <>
              <svg className="animate-spin" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3">
                <path d="M21 12a9 9 0 1 1-6.219-8.56"></path>
              </svg>
              Processing...
            </>
          ) : 'Execute Download'}
        </button>
      </div>

      <div className="card">
        <div
          className="card-title collapsible-title"
          onClick={() => setMetadataExpanded((prev: boolean) => !prev)}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="8" y1="6" x2="21" y2="6"></line>
              <line x1="8" y1="12" x2="21" y2="12"></line>
              <line x1="8" y1="18" x2="21" y2="18"></line>
              <line x1="3" y1="6" x2="3.01" y2="6"></line>
              <line x1="3" y1="12" x2="3.01" y2="12"></line>
              <line x1="3" y1="18" x2="3.01" y2="18"></line>
            </svg>
            Metadata & Progress
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
            {metadata.length > 0 && (
              <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)' }}>
                {filteredCount} of {metadata.length} records selected
              </span>
            )}
            <svg
              className={`chevron ${metadataExpanded ? 'expanded' : ''}`}
              viewBox="0 0 24 24"
              width="18"
              height="18"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </div>
        </div>

        {globalProgress && globalProgress.total > 0 && (
          <div className="global-progress">
            <div className="global-progress-header">
              <div>
                <span className="global-progress-label">Overall progress</span>
                <span className="global-progress-summary">
                  {globalProgress.completed} of {globalProgress.total} runs complete
                  {globalProgress.active > 0 && ` · ${globalProgress.active} active`}
                </span>
              </div>
              <div className="global-progress-metrics">
                {globalProgress.speed_mbps > 0 && (
                  <span>{globalProgress.speed_mbps.toFixed(1)} MB/s</span>
                )}
                <strong>{Math.round(globalProgress.percent)}%</strong>
              </div>
            </div>
            <div
              className="progress-bar-container progress-bar-global"
              role="progressbar"
              aria-label="Overall download progress"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(globalProgress.percent)}
            >
              <div
                className={`progress-bar ${isDownloading ? 'active' : ''}`}
                style={{ width: `${clampPercent(globalProgress.percent)}%` }}
              />
            </div>
          </div>
        )}

        {metadataExpanded && (
          metadata.length > 0 ? (
            <div className="metadata-container">
              <table>
                <thead>
                  <tr>
                    <th>Run Accession</th>
                    <th>Sample Title</th>
                    <th>Layout</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {metadata.filter(shouldInclude).map((record: EnaRecord) => (
                    <tr key={record.run_accession}>
                      <td style={{ fontWeight: 600, color: 'var(--primary)' }}>{record.run_accession}</td>
                      <td style={{ maxWidth: '300px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{record.sample_title}</td>
                      <td><span className="status-badge" style={{ backgroundColor: '#334155', color: '#f1f5f9' }}>{record.library_layout || 'N/A'}</span></td>
                      <td>
                        {progress[record.run_accession] ? (
                          <div className="run-progress">
                            <div className="run-progress-header">
                              <span className={`status-badge ${getStatusClass(progress[record.run_accession].status)}`}>
                                {progress[record.run_accession].status}
                              </span>
                              <span className="run-progress-metrics">
                                {progress[record.run_accession].speed_mbps > 0 && (
                                  <span className="run-progress-speed">
                                    {progress[record.run_accession].speed_mbps.toFixed(1)} MB/s
                                  </span>
                                )}
                                <strong>{Math.round(progress[record.run_accession].percent)}%</strong>
                              </span>
                            </div>
                            <div
                              className="progress-bar-container"
                              role="progressbar"
                              aria-label={`${record.run_accession} progress`}
                              aria-valuemin={0}
                              aria-valuemax={100}
                              aria-valuenow={Math.round(progress[record.run_accession].percent)}
                            >
                              <div
                                className={`progress-bar ${getProgressBarClass(progress[record.run_accession].status)}`}
                                style={{ width: `${clampPercent(progress[record.run_accession].percent)}%` }}
                              />
                            </div>
                            <div className="stage-track" aria-label={`Current stage: ${progress[record.run_accession].status}`}>
                              {['Download', 'Convert', 'Compress'].map((stage, index) => {
                                const stageIndex = getStageIndex(progress[record.run_accession].status);
                                const state = index < stageIndex
                                  ? 'done'
                                  : index === stageIndex && progress[record.run_accession].status !== 'Completed'
                                    ? 'current'
                                    : '';
                                return <span key={stage} className={state}>{stage}</span>;
                              })}
                            </div>
                          </div>
                        ) : (
                          <span className="status-badge" style={{ opacity: 0.5 }}>Pending</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="empty-state">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1">
                <path d="M20 14.66V20a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h5.34"></path>
                <polygon points="18 2 22 6 12 16 8 16 8 12 18 2"></polygon>
              </svg>
              <p>No metadata records loaded.</p>
              <p style={{ fontSize: '0.8rem' }}>Enter an accession or select a TSV file and click "Check Records".</p>
            </div>
          )
        )}
      </div>
    </div>
  );
}

// Upload Tab Component
function UploadTab({
  form,
  setForm,
  progress,
  isUploading,
  onStartUpload,
  selectFile,
}: any) {
  const handleSelectFiles = async () => {
    const selected = await selectFile(true);
    if (selected) {
      const files = Array.isArray(selected) ? selected : [selected];
      setForm((prev: any) => ({ ...prev, files }));
    }
  };

  return (
    <div className="upload-tab">
      <div className="card" style={{ marginBottom: '1.5rem', borderLeft: '4px solid #f59e0b', backgroundColor: 'rgba(245, 158, 11, 0.08)' }}>
        <div className="card-title" style={{ color: '#f59e0b' }}>
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
            <line x1="12" y1="9" x2="12" y2="13"></line>
            <line x1="12" y1="17" x2="12.01" y2="17"></line>
          </svg>
          Experimental Feature
        </div>
        <p style={{ fontSize: '0.875rem', color: 'var(--text-muted)', margin: 0 }}>
          The upload feature is still under testing. Please verify your uploads and use with caution.
        </p>
      </div>

      <div className="grid-2">
        <div className="card">
          <div className="card-title">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
            </svg>
            Destination (S3)
          </div>
          <div className="form-group">
            <label>Bucket Name</label>
            <input
              type="text"
              value={form.bucket}
              onChange={(e) => setForm((prev: any) => ({ ...prev, bucket: e.target.value }))}
              disabled={isUploading}
              placeholder="e.g., sequence-data-bucket"
            />
          </div>

          <div className="form-group">
            <label>Key Prefix (optional)</label>
            <input
              type="text"
              value={form.prefix}
              onChange={(e) => setForm((prev: any) => ({ ...prev, prefix: e.target.value }))}
              disabled={isUploading}
              placeholder="project-name/runs/"
            />
          </div>
          <div className="form-group">
            <label>Region</label>
            <input
              type="text"
              value={form.region}
              onChange={(e) => setForm((prev: any) => ({ ...prev, region: e.target.value }))}
              disabled={isUploading}
              placeholder="us-east-1"
            />
          </div>
        </div>

        <div className="card">
          <div className="card-title">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
              <polyline points="17 8 12 3 7 8"></polyline>
              <line x1="12" y1="3" x2="12" y2="15"></line>
            </svg>
            Files to Upload
          </div>
          <div className="form-group">
            <label>Select Files</label>
            <div className="form-row">
              <input
                type="text"
                value={form.files.length > 0 ? `${form.files.length} file(s) selected` : ''}
                readOnly
                placeholder="No files selected"
                style={{ flex: 1 }}
              />
              <button className="btn btn-secondary" onClick={handleSelectFiles} disabled={isUploading}>
                Browse
              </button>
            </div>
          </div>
          <div className="form-group">
            <label>Concurrent Uploads</label>
            <input
              type="number"
              min={1}
              value={form.concurrent}
              onChange={(e) => setForm((prev: any) => ({ ...prev, concurrent: parseInt(e.target.value) }))}
              disabled={isUploading}
            />
          </div>
          <div style={{ display: 'flex', gap: '1rem', marginTop: '1rem' }}>
            <label className="checkbox-group">
              <input
                type="checkbox"
                checked={form.applyPolicy}
                onChange={(e) => setForm((prev: any) => ({ ...prev, applyPolicy: e.target.checked }))}
                disabled={isUploading}
              />
              <span>NCBI Policy</span>
            </label>
            <label className="checkbox-group">
              <input
                type="checkbox"
                checked={form.dryRun}
                onChange={(e) => setForm((prev: any) => ({ ...prev, dryRun: e.target.checked }))}
                disabled={isUploading}
              />
              <span>Dry Run</span>
            </label>
          </div>
        </div>
      </div>

      <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: '2rem' }}>
        <button
          className="btn btn-primary"
          onClick={onStartUpload}
          disabled={isUploading || !form.bucket || form.files.length === 0}
        >
          {isUploading ? 'Uploading...' : 'Initiate Upload'}
        </button>
      </div>

      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path>
            <polyline points="22 4 12 14.01 9 11.01"></polyline>
          </svg>
          Upload Progress
        </div>
        {form.files.length > 0 ? (
          <div className="progress-list">
            {form.files.map((file: string, i: number) => {
              const filename = file.split(/[/\\]/).pop() || file;
              const item = progress[filename];
              return (
                <div key={i} className="progress-item">
                  <div className="progress-header">
                    <span style={{ fontWeight: 600 }}>{filename}</span>
                    <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center' }}>
                      {item ? (
                        <>
                          <span className={`status-badge ${getStatusClass(item.status)}`}>{item.status}</span>
                          <span>{Math.round(item.percent)}%</span>
                        </>
                      ) : (
                        <span className="status-badge" style={{ opacity: 0.5 }}>Waiting</span>
                      )}
                    </div>
                  </div>
                  <div className="progress-bar-container">
                    <div
                      className={`progress-bar ${item ? getProgressBarClass(item.status) : ''}`}
                      style={{ width: `${item ? item.percent : 0}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
              <polyline points="17 8 12 3 7 8"></polyline>
              <line x1="12" y1="3" x2="12" y2="15"></line>
            </svg>
            <p>No files queued for upload.</p>
            <p style={{ fontSize: '0.8rem' }}>Browse and select local files to begin.</p>
          </div>
        )}
      </div>
    </div>
  );
}

// Settings Tab Component
function SettingsTab({
  form,
  setForm,
  onSaveConfig,
  selectFile,
  configPath,
  onSelectConfigPath,
}: any) {
  const createFileSelector = (field: string) => async () => {
    const selected = await selectFile(false);
    if (selected && !Array.isArray(selected)) {
      setForm((prev: any) => ({ ...prev, [field]: selected }));
    }
  };

  return (
    <div className="settings-tab">
      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 0 0 2.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 0 0 1.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 0 0-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 0 0-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 0 0-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 0 0-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 0 0 1.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"></path>
            <circle cx="12" cy="12" r="3"></circle>
          </svg>
          Configuration File
        </div>
        <div className="form-group">
          <label>Config File Path</label>
          <div className="form-row">
            <input
              type="text"
              value={configPath}
              readOnly
              placeholder="Path to EBIDownload.yaml"
              style={{ flex: 1 }}
            />
            <button className="btn btn-secondary" onClick={onSelectConfigPath}>
              Browse
            </button>
          </div>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-muted)', marginTop: '0.5rem' }}>
            Default location is your system config directory. Saving will create the file if it does not exist.
          </p>
        </div>
      </div>

      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"></path>
          </svg>
          Software Executables
        </div>
        <div className="form-group">
          <label>NCBI Prefetch Path</label>
          <div className="form-row">
            <input
              type="text"
              value={form.prefetchPath}
              onChange={(e) => setForm((prev: any) => ({ ...prev, prefetchPath: e.target.value }))}
              placeholder="e.g., /usr/local/bin/prefetch"
              style={{ flex: 1 }}
            />
            <button className="btn btn-secondary" onClick={createFileSelector('prefetchPath')}>Browse</button>
          </div>
        </div>

        <div className="form-group">
          <label>fasterq-dump Path</label>
          <div className="form-row">
            <input
              type="text"
              value={form.fasterqDumpPath}
              onChange={(e) => setForm((prev: any) => ({ ...prev, fasterqDumpPath: e.target.value }))}
              placeholder="e.g., /usr/local/bin/fasterq-dump"
              style={{ flex: 1 }}
            />
            <button className="btn btn-secondary" onClick={createFileSelector('fasterqDumpPath')}>Browse</button>
          </div>
        </div>
        
        <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: '1rem' }}>
          <button className="btn btn-primary" onClick={onSaveConfig}>
            Save Configuration
          </button>
        </div>
      </div>
    </div>
  );
}

// About Tab Component
function AboutTab() {
  return (
    <div className="about-tab">
      <div className="card about-hero">
        <div className="about-logo">
          <svg viewBox="0 0 24 24" width="48" height="48" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
            <polyline points="7 10 12 15 17 10"></polyline>
            <line x1="12" y1="15" x2="12" y2="3"></line>
          </svg>
        </div>
        <h1>EBIDownload</h1>
        <p className="about-version">Version 1.4.1</p>
        <p className="about-description">
          A high-performance tool for downloading sequencing data from ENA/SRA.
          Supports multiple protocols and automatic format conversion.
        </p>
        <div className="about-links">
          <a
            className="about-link"
            href="https://github.com/xsx123123/EBIDownload"
            target="_blank"
            rel="noopener noreferrer"
          >
            <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden="true">
              <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56 0-.28-.01-1.02-.02-2-3.2.7-3.87-1.54-3.87-1.54-.52-1.32-1.27-1.67-1.27-1.67-1.04-.71.08-.7.08-.7 1.15.08 1.75 1.18 1.75 1.18 1.02 1.75 2.69 1.24 3.34.95.1-.74.4-1.24.72-1.53-2.55-.29-5.24-1.28-5.24-5.7 0-1.26.45-2.29 1.18-3.1-.12-.29-.51-1.46.11-3.05 0 0 .97-.31 3.18 1.18a11.04 11.04 0 0 1 5.78 0c2.21-1.49 3.18-1.18 3.18-1.18.62 1.59.23 2.76.11 3.05.74.81 1.18 1.84 1.18 3.1 0 4.43-2.69 5.41-5.25 5.69.41.36.78 1.06.78 2.13 0 1.54-.01 2.78-.01 3.16 0 .31.21.68.8.56C20.21 21.39 23.5 17.08 23.5 12 23.5 5.65 18.35.5 12 .5z"/>
            </svg>
            View on GitHub
          </a>
        </div>
        <blockquote className="cosmic-quote">
          “We are only borrowing these carbon, hydrogen, and oxygen atoms from the universe for a few decades,
          using them to briefly experience this world. Every atom that makes up our brain and body comes from
          the nuclear fusion explosions inside ancient stars billions of years ago. When life ends, we do not
          vanish into nothingness — we simply return to the vast universe and continue to exist in another form.”
        </blockquote>
      </div>

      <div className="card">
        <div className="card-title">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path>
          </svg>
          License & Credits
        </div>
        <p style={{ fontSize: '0.9rem', color: 'var(--text-muted)', lineHeight: 1.6 }}>
          Licensed under MIT. Built with Rust, Tauri, and React.
        </p>
      </div>
    </div>
  );
}

export default App;
