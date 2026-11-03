//! 垃圾扫描验证工具（开发用，不随应用发布）。
//!
//! 用法：cargo run --release -p purimaster-rules --example junk_scan -- [规则目录]
//!
//! 作用：加载真实规则文件，在本机执行一次只读扫描，打印按规则分组的统计，
//! 用于验证规则数据与保护逻辑在真实系统上的表现。

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use purimaster_rules::RuleSet;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../rules"));

    let set = match RuleSet::load_dir(&dir) {
        Ok(set) => set,
        Err(e) => {
            eprintln!("加载规则失败（{}）：{e}", dir.display());
            std::process::exit(1);
        }
    };
    println!(
        "规则目录：{}\n加载 {} 条规则 / {} 个分类",
        dir.display(),
        set.rules().len(),
        set.categories().len()
    );

    let result = match set.scan(&AtomicBool::new(false)) {
        Ok(r) => r,
        Err(_) => {
            eprintln!("扫描被取消");
            return;
        }
    };

    println!(
        "\n合计：{} 个文件，{:.2} MB，跳过 {} 项\n",
        result.total_files,
        result.total_bytes as f64 / 1048576.0,
        result.skipped
    );

    for group in &result.groups {
        println!(
            "[{}] {}（{}，{} 文件，{:.2} MB）",
            group.category,
            group.title,
            match group.risk {
                purimaster_rules::Risk::Low => "低风险",
                purimaster_rules::Risk::Medium => "中风险",
                purimaster_rules::Risk::High => "高风险",
            },
            group.files,
            group.bytes as f64 / 1048576.0
        );
        for target in &group.targets {
            println!("    目标：{target}");
        }
        for problem in &group.problems {
            println!("    ⚠ {problem}");
        }
        for file in group.top_files.iter().take(3) {
            println!("    {:>12} B  {}", file.size, file.path);
        }
    }
}
