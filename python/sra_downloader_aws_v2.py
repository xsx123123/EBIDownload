#!/usr/bin/env python3
# -*- coding: utf-8 -*-

"""
SRA Pro Downloader V2.1
功能：NCBI SRA 数据高速下载工具 (基于 AWS S3)
特性：断点续传、MD5校验、智能重试、详细日志、文件级并发
"""

import os
import sys
import re
import json
import time
import threading
import hashlib
import argparse
import requests
import boto3
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor, ProcessPoolExecutor, as_completed
from botocore import UNSIGNED
from botocore.config import Config
from tqdm import tqdm
from loguru import logger
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type
from rich.console import Console
from rich_argparse import RichHelpFormatter

# ============================
# 1. 配置与工具函数
# ============================

def setup_logging(log_file="sra_download.log"):
    """配置日志：屏幕显示INFO，文件记录DEBUG"""
    logger.remove()
    # 控制台输出：简洁格式
    logger.add(sys.stderr, format="<green>{time:HH:mm:ss}</green> | <level>{message}</level>", level="INFO")
    # 文件输出：详细格式，支持轮转
    logger.add(log_file, rotation="10 MB", retention="7 days", level="DEBUG", 
               format="{time:YYYY-MM-DD HH:mm:ss} | {level} | {process.name} | {message}")

def calculate_md5(file_path):
    """流式计算大文件 MD5，避免内存溢出"""
    hash_md5 = hashlib.md5()
    try:
        with open(file_path, "rb") as f:
            # 每次读取 8MB
            for chunk in iter(lambda: f.read(8 * 1024 * 1024), b""):
                hash_md5.update(chunk)
        return hash_md5.hexdigest()
    except FileNotFoundError:
        return None

def format_speed(size_bytes, duration_seconds):
    """计算并格式化速度"""
    if duration_seconds <= 0: return "N/A"
    speed = (size_bytes / 1024 / 1024) / duration_seconds
    return f"{speed:.2f} MB/s"

def format_time(seconds):
    """格式化时间"""
    return f"{seconds:.2f} s"

# ============================
# 2. 核心类定义
# ============================

class SraMetadata:
    def __init__(self, s3_uri, md5, size):
        self.s3_uri = s3_uri
        self.md5 = md5
        self.size = int(size)

class SraUtils:
    @staticmethod
    @retry(stop=stop_after_attempt(3), wait=wait_exponential(multiplier=1, min=2, max=10))
    def get_metadata(run_id, api_key=None):
        """获取 S3 地址和 MD5 值 (带重试机制)"""
        url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi"
        params = {"db": "sra", "id": run_id, "rettype": "full", "retmode": "xml"}
        if api_key:
            params["api_key"] = api_key
        
        logger.info(f"[{run_id}] 正在获取元数据...")
        resp = requests.get(url, params=params, timeout=30)
        resp.raise_for_status()
        
        root = ET.fromstring(resp.text)
        
        # 1. 寻找 S3 链接
        target_url = None
        for alt in root.findall(".//Alternatives"):
            if alt.get('org') == 'AWS' and alt.get('free_egress') == 'worldwide':
                target_url = alt.get('url')
                break
        
        if not target_url:
            logger.warning(f"[{run_id}] 未找到明确的 AWS worldwide 链接")
            return None

        # 转换 s3 链接
        match = re.match(r'https://([^.]+)\.s3\.amazonaws\.com/(.+)', target_url)
        if match:
            s3_uri = f"s3://{match.group(1)}/{match.group(2)}"
        else:
            s3_uri = target_url.replace("https://", "s3://").replace(".s3.amazonaws.com", "")

        # 2. 寻找 MD5 和 文件大小
        expected_md5 = None
        file_size = 0
        
        filename = s3_uri.split('/')[-1]
        for sra_file in root.findall(".//SRAFile"):
            if sra_file.get('filename') == filename:
                expected_md5 = sra_file.get('md5')
                file_size = sra_file.get('size')
                break
        
        if not file_size:
             # 备用方案：尝试取 Run 节点信息
             run_node = root.find(".//RUN")
             if run_node is not None:
                 file_size = run_node.get('size', 0)

        logger.debug(f"[{run_id}] 元数据解析成功: Size={file_size}, MD5={expected_md5}")
        return SraMetadata(s3_uri, expected_md5, file_size)

class ResumableDownloader:
    def __init__(self, run_id, metadata, save_dir=".", chunk_size_mb=20, max_workers=8):
        self.run_id = run_id
        self.metadata = metadata
        self.bucket, self.key = self._parse_s3_uri(metadata.s3_uri)
        self.filename = os.path.basename(self.key)
        self.save_dir = save_dir
        self.filepath = os.path.join(save_dir, self.filename)
        self.meta_file = self.filepath + ".meta.json"
        
        self.chunk_size = chunk_size_mb * 1024 * 1024
        self.max_workers = max_workers
        self.s3_client = boto3.client('s3', config=Config(signature_version=UNSIGNED, max_pool_connections=max_workers))
        self.file_lock = threading.Lock()

        if not os.path.exists(self.save_dir):
            os.makedirs(self.save_dir)

    def _parse_s3_uri(self, uri):
        match = re.match(r's3://([^/]+)/(.+)', uri)
        if match:
            return match.group(1), match.group(2)
        raise ValueError(f"Invalid S3 URI: {uri}")

    def _load_progress(self):
        if os.path.exists(self.meta_file):
            try:
                with open(self.meta_file, 'r') as f:
                    return json.load(f)
            except: 
                pass
        return {"downloaded_chunks": []}

    def _save_progress(self, downloaded_chunks):
        with open(self.meta_file, 'w') as f:
            json.dump({"downloaded_chunks": list(downloaded_chunks)}, f)

    @retry(stop=stop_after_attempt(5), wait=wait_exponential(multiplier=1, min=1, max=10), 
           retry=retry_if_exception_type((requests.ConnectionError, boto3.exceptions.Boto3Error)))
    def _download_chunk_task(self, start, end, chunk_id, pbar):
        """下载分片（带重试）"""
        range_header = f"bytes={start}-{end}"
        resp = self.s3_client.get_object(Bucket=self.bucket, Key=self.key, Range=range_header)
        content = resp['Body'].read()
        
        with self.file_lock:
            with open(self.filepath, 'r+b') as f:
                f.seek(start)
                f.write(content)
        
        pbar.update(len(content))
        return chunk_id

    def start(self):
        start_time = time.time()
        logger.info(f"[{self.run_id}] 准备下载: {self.filename} ({self.metadata.size / (1024**3):.2f} GB)")
        
        # 1. 预分配文件
        if not os.path.exists(self.filepath):
            with open(self.filepath, 'wb') as f:
                f.truncate(self.metadata.size)
        
        # 2. 计算分片任务
        total_size = self.metadata.size
        num_chunks = (total_size + self.chunk_size - 1) // self.chunk_size
        progress_data = self._load_progress()
        downloaded_chunks = set(progress_data['downloaded_chunks'])
        
        tasks = []
        for i in range(num_chunks):
            if i in downloaded_chunks: continue
            start = i * self.chunk_size
            end = min(start + self.chunk_size - 1, total_size - 1)
            tasks.append((i, start, end))
            
        initial_downloaded = sum([self.chunk_size for _ in downloaded_chunks])
        if initial_downloaded > total_size: initial_downloaded = total_size

        if not tasks:
            logger.info(f"[{self.run_id}] 文件已存在，跳过下载，直接校验...")
            return self._verify_integrity(start_time, skipped_download=True)

        logger.info(f"[{self.run_id}] 剩余分片: {len(tasks)}/{num_chunks} | 线程: {self.max_workers}")

        # 进度条
        pbar = tqdm(total=total_size, initial=initial_downloaded, unit='B', unit_scale=True, 
                   desc=f"{self.run_id}", ncols=100, position=0, leave=True)
        
        with ThreadPoolExecutor(max_workers=self.max_workers) as executor:
            future_to_chunk = {
                executor.submit(self._download_chunk_task, s, e, i, pbar): i 
                for i, s, e in tasks
            }
            
            completed_count = 0
            for future in as_completed(future_to_chunk):
                try:
                    chunk_id = future.result()
                    downloaded_chunks.add(chunk_id)
                    completed_count += 1
                    # 每10个分片保存一次进度，减少IO
                    if completed_count % 10 == 0:
                        self._save_progress(downloaded_chunks)
                except Exception as e:
                    tqdm.write(f"❌ [{self.run_id}] 分片下载失败: {e}")
        
        pbar.close()
        self._save_progress(downloaded_chunks)

        if len(downloaded_chunks) == num_chunks:
            return self._verify_integrity(start_time, skipped_download=False)
        else:
            logger.error(f"[{self.run_id}] 下载未完成，部分分片失败。")
            return False

    def _verify_integrity(self, start_time, skipped_download=False):
        """校验 MD5 并记录最终日志"""
        download_end_time = time.time()
        download_duration = download_end_time - start_time
        
        if not self.metadata.md5:
            logger.warning(f"[{self.run_id}] XML未提供MD5，跳过校验。")
            logger.success(f"[{self.run_id}] 任务完成 (无校验). 耗时: {format_time(download_duration)}")
            if os.path.exists(self.meta_file): os.remove(self.meta_file)
            return True

        logger.info(f"[{self.run_id}] 正在校验 MD5 (本地计算中)...")
        verify_start = time.time()
        local_md5 = calculate_md5(self.filepath)
        verify_duration = time.time() - verify_start
        
        if local_md5 == self.metadata.md5:
            speed_info = ""
            if not skipped_download:
                speed_str = format_speed(self.metadata.size, download_duration)
                speed_info = f" | 速度: {speed_str}"
            
            logger.success(f"[{self.run_id}] ✅ 校验通过! 总耗时: {format_time(download_duration + verify_duration)} (下载: {format_time(download_duration)}{speed_info}, 校验: {format_time(verify_duration)})")
            
            # 记录到日志文件供统计
            logger.debug(f"STATS | ID={self.run_id} | Size={self.metadata.size} | Time={download_duration:.2f}s | Speed={speed_info}")
            
            if os.path.exists(self.meta_file): os.remove(self.meta_file)
            return True
        else:
            logger.critical(f"[{self.run_id}] ❌ 校验失败! 本地:{local_md5} != 远程:{self.metadata.md5}")
            return False

# ============================
# 3. 任务调度逻辑
# ============================

def process_single_run(run_id, args):
    """单个下载任务的入口函数（供多进程调用）"""
    try:
        # 1. 获取元数据
        metadata = SraUtils.get_metadata(run_id, api_key=args.api_key)
        if not metadata:
            logger.error(f"[{run_id}] 无法获取有效的下载链接")
            return False
        
        # 2. 启动下载
        downloader = ResumableDownloader(
            run_id=run_id,
            metadata=metadata,
            save_dir=args.outdir,
            max_workers=args.threads,     # 每个文件内部的分片下载线程数
            chunk_size_mb=args.chunk_size
        )
        return downloader.start()
        
    except Exception as e:
        logger.exception(f"[{run_id}] 处理过程中发生未知错误")
        return False

# ============================
# 4. CLI 主入口
# ============================

def main():
    # 局部导入，确保只有在运行时才需要这些库
    try:
        from rich.console import Console
        from rich.panel import Panel
        from rich.text import Text
        from rich_argparse import RichHelpFormatter
    except ImportError:
        print("为了获得最佳体验，请安装美化库: pip install rich rich-argparse")
        sys.exit(1)

    # --- 1. 定义 Help 界面样式 ---
    # 让参数名显示为青色，分组标题显示为加粗黄色
    RichHelpFormatter.styles["argparse.args"] = "cyan"
    RichHelpFormatter.styles["argparse.groups"] = "bold yellow"
    RichHelpFormatter.styles["argparse.help"] = "white"
    RichHelpFormatter.styles["argparse.metavar"] = "bold magenta"

    parser = argparse.ArgumentParser(
        description="[bold green]NCBI SRA Pro Downloader V2.1[/]\n"
                    "基于 AWS S3 的高速生物数据下载器，支持 [bold red]断点续传[/] & [bold red]MD5校验[/]",
        formatter_class=RichHelpFormatter,
        epilog="[dim]Example: python sra_download.py SRR32730731 -o ./data -p 4[/]"
    )

    # --- 2. 参数分组 (让 -h 界面更清晰) ---
    
    # [核心参数]
    req_group = parser.add_argument_group("🔥 核心参数")
    req_group.add_argument("ids", nargs="+", help="SRA Run IDs (支持多个，例如: [green]SRR32730731 SRR32730732[/])")

    # [常用选项]
    opt_group = parser.add_argument_group("📂 常用选项")
    opt_group.add_argument("-o", "--outdir", default=".", 
                           help="下载保存目录 (默认: [u]./[/])")
    
    # [性能调优]
    perf_group = parser.add_argument_group("🚀 性能调优")
    perf_group.add_argument("-p", "--parallel-files", type=int, default=1, metavar="N",
                           help="[bold]文件级并发数[/]：同时下载的文件数量 (默认: 1, 即串行)")
    perf_group.add_argument("-t", "--threads", type=int, default=8, metavar="N",
                           help="[bold]线程级并发数[/]：单文件内部分片下载线程数 (默认: 8)")
    perf_group.add_argument("--chunk-size", type=int, default=20, metavar="MB",
                           help="分片大小 (默认: 20 MB)")

    # [其他设置]
    misc_group = parser.add_argument_group("⚙️ 其他设置")
    misc_group.add_argument("--api-key", metavar="KEY", 
                           help="NCBI API Key (用于提升 API 限流阈值)")
    misc_group.add_argument("--log", default="sra_download.log", metavar="FILE",
                           help="日志文件路径")

    args = parser.parse_args()
    
    # --- 3. 初始化与 Banner 显示 ---
    
    setup_logging(args.log)
    
    # 确保输出目录存在
    if not os.path.exists(args.outdir):
        os.makedirs(args.outdir)
        
    console = Console()
    
    # 显示启动面板
    summary_text = Text()
    summary_text.append(f"📦 待下载文件: {len(args.ids)} 个\n", style="bold white")
    summary_text.append(f"📂 保存目录: {os.path.abspath(args.outdir)}\n", style="cyan")
    summary_text.append(f"🚀 并发配置: {args.parallel_files} 文件 x {args.threads} 线程", style="green")
    
    console.print(Panel(summary_text, title="[bold green]SRA Downloader 任务启动[/]", expand=False))

    start_time_all = time.time()
    total_files = len(args.ids)
    
    # 记录到 Loguru 文件日志 (保留技术细节)
    logger.info(f"CLI启动参数: {args}")

    failed_ids = []
    
    # --- 4. 任务执行逻辑 (串行 vs 并行) ---
    
    try:
        if args.parallel_files > 1 and total_files > 1:
            # === 多进程并行下载 ===
            console.print(f"[bold yellow]⚡ 启用多进程模式 (Pool Size: {args.parallel_files})[/]")
            
            with ProcessPoolExecutor(max_workers=args.parallel_files) as executor:
                # 提交任务
                future_to_id = {executor.submit(process_single_run, run_id, args): run_id for run_id in args.ids}
                
                for future in as_completed(future_to_id):
                    run_id = future_to_id[future]
                    try:
                        success = future.result()
                        if not success:
                            failed_ids.append(run_id)
                    except Exception as e:
                        logger.error(f"[{run_id}] 进程异常: {e}")
                        failed_ids.append(run_id)
        else:
            # === 串行下载 ===
            if total_files > 1:
                console.print("[dim]提示: 使用 -p 参数可开启多文件并行下载[/]")
                
            for i, run_id in enumerate(args.ids, 1):
                console.print(f"\n[bold]Processing {i}/{total_files}: {run_id}[/]")
                success = process_single_run(run_id, args)
                if not success:
                    failed_ids.append(run_id)

    except KeyboardInterrupt:
        console.print("\n[bold red]⚠️ 用户中断操作！正在安全退出...[/]")
        sys.exit(130)

    # --- 5. 最终统计 ---
    total_duration = time.time() - start_time_all
    
    console.print("\n")
    if failed_ids:
        console.print(Panel(f"❌ 失败列表: {', '.join(failed_ids)}", title="任务部分失败", style="bold red"))
    else:
        console.print(Panel(f"✅ 所有 {total_files} 个文件下载成功！\n⏱️ 总耗时: {format_time(total_duration)}", 
                            title="任务完成", style="bold green"))

if __name__ == "__main__":
    main()