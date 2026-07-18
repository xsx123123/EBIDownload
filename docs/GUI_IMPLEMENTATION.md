# Polariseq GUI Implementation

## Overview

This document describes the Tauri-based GUI implementation for Polariseq.

## Architecture

```
Polariseq/
├── src/                          # React frontend
│   ├── App.tsx                  # Main application component
│   ├── main.tsx                 # React entry point
│   ├── styles.css               # Styling
│   └── index.html               # HTML template
├── src-tauri/                   # Rust backend
│   ├── src/
│   │   ├── lib.rs               # Core library (reusable code)
│   │   ├── main.rs              # Tauri entry point
│   │   ├── aws_s3.rs            # AWS S3 download module
│   │   ├── ftp.rs               # FTP/Aspera download module
│   │   ├── prefetch.rs          # Prefetch download module
│   │   ├── progress.rs          # Progress bar utilities
│   │   └── upload_module/       # Upload module
│   ├── Cargo.toml               # Rust dependencies
│   ├── build.rs                 # Tauri build script
│   └── tauri.conf.json          # Tauri configuration
├── package.json                 # Node.js dependencies
├── tsconfig.json                # TypeScript configuration
└── vite.config.ts               # Vite configuration
```

## Current Status

### ✅ Completed

1. **Project Structure** - Created Tauri + React project skeleton
2. **Core Library** - Extracted core functionality into `lib.rs` with public API
3. **React Frontend** - Created main UI with:
   - Download tab (accession, TSV, options)
   - Upload tab (S3 bucket, files, options)
   - Settings tab (config file paths)
   - Real-time logging
4. **Tauri Commands** - Implemented basic command handlers

### 🔄 Next Steps

To fully implement the GUI, follow these steps:

### 1. Install Dependencies

```bash
# Install Node.js dependencies
npm install

# Install Tauri CLI
cargo install tauri-cli
```

### 2. Verify Original CLI Still Works

Keep the original CLI functional by maintaining it as a separate build or branch.

### 3. Complete Tauri Integration

- Add missing plugin dependencies to `Cargo.toml`
- Implement proper progress event system
- Connect real download/upload functions
- Add proper error handling
- Implement file dialogs

### 4. Build and Test

```bash
# Development mode
npm run tauri dev

# Production build
npm run tauri build
```

## Frontend Features

### Download Tab
- Accession input or TSV file selection
- Output directory picker
- Download method selection (AWS, Aspera, FTP, Prefetch, Auto)
- Concurrency settings
- PE-only filter
- Dry run option
- Real-time progress display
- Metadata preview

### Upload Tab
- S3 bucket configuration
- File selection (multiple files)
- Region selection
- Concurrent uploads
- Policy application option
- Metadata template generation
- Progress tracking

### Settings Tab
- Config file path management
- Tool path validation
- Real-time config editing

## Backend API

### Tauri Commands

```rust
// Config management
get_config_command() -> Option<Config>
load_config_command(path: Option<String>) -> Result<()>
save_config_command(config: ConfigInput) -> Result<()>

// Metadata
fetch_metadata_command(accession: Option<String>, tsv: Option<String>) -> Result<Vec<EnaRecord>>

// Downloads
start_download_command(options: DownloadOptions) -> Result<()>

// Uploads
start_upload_command(options: UploadOptions) -> Result<()>
```

### Event System

The backend emits events to update the UI:
- `download-event`: Logs, progress, status
- `upload-event`: Upload logs and progress

## Key Decisions

### Why Tauri?
- Small bundle size (~10MB vs ~100MB for Electron)
- Rust backend for performance
- Reuse existing Rust code
- Cross-platform support

### Why React?
- Component-based architecture
- Large ecosystem
- TypeScript support
- Good integration with Tauri

## Migrating from CLI

The existing CLI logic is preserved in the library:
- All download/upload functions are still available
- Config format is unchanged
- Metadata handling is preserved
- Just wrapped with Tauri commands and React UI

## Development Tips

1. **Hot Reload**: Tauri dev mode auto-reloads both frontend and backend
2. **Dev Tools**: Enable web dev tools in Tauri config for debugging
3. **Logging**: Use `tauri-plugin-log` for unified logging
4. **Testing**: Test download/upload logic in CLI first, then connect to GUI

## Next Steps

To complete the implementation:

1. **Finish Backend Integration**
   - Connect actual download functions in Tauri commands
   - Implement progress event streaming
   - Add proper error handling
   - Implement file dialogs

2. **Finish Frontend**
   - Add state management (useReducer, Zustand, etc.)
   - Improve styling
   - Add loading states
   - Add confirmation dialogs
   - Implement table sorting/filtering

3. **Testing**
   - Test all download modes
   - Test upload functionality
   - Test config management
   - Cross-platform testing

4. **Packaging**
   - Create installers
   - Code signing
   - Auto-update setup
