//! 真实磁盘扫描验证工具（开发用，不随应用发布）。
//!
//! 用法：cargo run --release -p purimaster-core --example scan_real -- <目录> [N秒后取消]
//!
//! 作用：
//! 1. 验证扫描引擎在真实大目录上进度回调正常触发（每 0.5s 打印一次）
//! 2. 验证取消标志在真实扫描中即时生效
//! 3. 顺手测量扫描吞吐与索引内存形态，作为性能基线

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use purimaster_core::{ScanError, ScanProgress, scan_space};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "C:\\".into());
    let cancel_after: Option<u64> = std::env::args().nth(2).and_then(|s| s.parse().ok());

    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(secs) = cancel_after {
        let cancel = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            eprintln!("--- 发出取消信号 ---");
            cancel.store(true, Ordering::Relaxed);
        });
    }

    let start = Instant::now();
    let mut last_print = Instant::now() - Duration::from_secs(1);
    let mut prints = 0u32;
    let mut progress = |p: ScanProgress| {
        if last_print.elapsed() >= Duration::from_millis(500) {
            last_print = Instant::now();
            prints += 1;
            eprintln!(
                "[{:>6.1}s] {:>9} 文件  {:>8.2} GB",
                start.elapsed().as_secs_f32(),
                p.files_seen,
                p.bytes_seen as f64 / 1073741824.0
            );
        }
    };

    let result = scan_space(Path::new(&root), &cancel, &mut progress);
    let elapsed = start.elapsed().as_secs_f32();
    match result {
        Ok(index) => {
            let root_node = index.summary(index.root_id()).unwrap();
            println!(
                "完成，用时 {elapsed:.1}s：{} 文件 / {} 子目录 / {:.2} GB / 跳过 {}（进度打印 {prints} 次）",
                root_node.files,
                root_node.dirs,
                index.bytes_seen as f64 / 1073741824.0,
                index.skipped
            );
            println!("索引节点数：{}", index.node_count());
            if root_node.unreadable {
                println!("注意：根目录不可读");
            }
            for c in index.children(index.root_id()).iter().take(5) {
                println!(
                    "  {:>8.2} GB  {}{}",
                    c.size as f64 / 1073741824.0,
                    c.name,
                    if c.unreadable { "（无权限）" } else { "" }
                );
            }
        }
        Err(ScanError::Cancelled) => {
            println!("已取消，用时 {elapsed:.1}s（取消前进度打印 {prints} 次，说明回调在正常触发）")
        }
        Err(ScanError::Io(e)) => eprintln!("IO 错误: {e}"),
    }
}
