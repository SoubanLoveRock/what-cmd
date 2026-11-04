//! 超时路径的日志断言。
//!
//! 放在独立的集成测试二进制中，且只含这一个测试。tracing 的 callsite 兴趣缓存是全局的，
//! 与其他并行测试放在同一进程时，日志会随别的测试的订阅器状态丢失，因此不放进单元测试。

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;
use whatcmd::process::{ProcError, run_finished};

/// 测试用日志写入端，内容放在共享缓冲区中供断言。
#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 在线程局部的订阅器下运行 `f`，返回结果与捕获到的日志文本。
fn capture_logs<T>(level: Level, f: impl FnOnce() -> T) -> (T, String) {
    let buf = LogBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(level)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    (out, text)
}

#[test]
fn timeout_logs_warn_with_name_and_limit() {
    let (result, logs) = capture_logs(Level::DEBUG, || {
        run_finished(
            Path::new("/bin/sh"),
            &["-c", "sleep 30"],
            &[],
            Duration::from_millis(200),
            1024,
        )
    });
    assert!(
        matches!(result, Err(ProcError::Timeout(200))),
        "got {result:?}"
    );
    assert!(logs.contains("WARN"), "no WARN line in logs: {logs}");
    assert!(
        logs.contains("command timed out, killing process group"),
        "warn message missing: {logs}"
    );
    assert!(
        logs.contains("timeout_ms=200"),
        "limit field missing: {logs}"
    );
    assert!(
        logs.contains("timeout reached"),
        "debug trail missing: {logs}"
    );
    assert!(
        logs.contains("kill group sent"),
        "debug trail missing: {logs}"
    );
}
