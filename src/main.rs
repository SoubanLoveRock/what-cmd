//! whatcmd 命令行入口：解析参数，调用库，映射退出码。不含业务逻辑。

use clap::Parser;

fn main() {
    // 日志只写 stderr，默认级别 warn。`-v` 与 `WHATCMD_LOG` 尚未接入。
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();
    let cli = whatcmd::cli::Cli::parse();
    std::process::exit(whatcmd::cli::run(cli).code());
}
