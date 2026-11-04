//! 带超时的子进程运行。
//!
//! 子进程在独立进程组中启动，stdin 为 `/dev/null`，输出读取有上限。超时或正常退出后，
//! 通过 `kill -KILL -- -<pgid>` 清理整个进程组，不经过 shell，也不引入 libc。
//! `docs` 与 `agent::claude` 共用这里的逻辑，保证超时与清理行为一致。

use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

/// 等待子进程的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 读取原始输出的上限倍数（相对调用方给出的 `max_bytes`）。超出部分读取后丢弃，避免子进程因管道阻塞。
const RAW_CAP_FACTOR: u64 = 4;

/// 子进程运行错误。
#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    /// 子进程超过时限，进程组已被杀死。
    #[error("timed out after {0} ms")]
    Timeout(u64),
    /// 启动或等待子进程失败。
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// 子进程正常结束后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    /// 退出码；被信号终止时为 `None`。
    pub code: Option<i32>,
    /// 标准输出，读取上限内的部分。
    pub stdout: Vec<u8>,
    /// 标准错误，读取上限内的部分。
    pub stderr: Vec<u8>,
}

/// 在独立进程组中运行 `program args...`，设置 `env`，等待结束并返回退出码与输出。
///
/// 超时返回 [`ProcError::Timeout`]，并杀死整个进程组。正常退出后也会清理组内残留进程。
pub fn run_finished(
    program: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Finished, ProcError> {
    debug!(program = %program.display(), ?args, timeout_ms = timeout.as_millis() as u64, "run command");
    let mut child = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let pgid = child.id();
    let cap = (max_bytes as u64).saturating_mul(RAW_CAP_FACTOR);
    let out_reader = drain(child.stdout.take(), cap);
    let err_reader = drain(child.stderr.take(), cap);

    let started = Instant::now();
    let status = loop {
        let polled = child.try_wait()?;
        if let Some(status) = polled {
            break status;
        }
        if started.elapsed() >= timeout {
            let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            warn!(name = %program.display(), timeout_ms = ms, "command timed out, killing process group");
            debug!(
                pgid,
                elapsed_ms,
                try_wait_running = polled.is_none(),
                "timeout reached"
            );
            kill_group(pgid);
            debug!(pgid, "kill group sent");
            let _ = child.wait();
            return Err(ProcError::Timeout(ms));
        }
        thread::sleep(POLL_INTERVAL);
    };
    // 子进程可能留下后台进程仍占着管道，正常退出后也要清理整个进程组。
    kill_group(pgid);
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(Finished {
        code: status.code(),
        stdout,
        stderr,
    })
}

/// 运行子进程并收集输出，只关心输出内容。
///
/// 返回 `Ok(None)` 表示 stdout 与 stderr 都为空。超时与清理规则同 [`run_finished`]。
pub fn run_captured(
    program: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, ProcError> {
    let done = run_finished(program, args, env, timeout, max_bytes)?;
    let bytes = if done.stdout.is_empty() {
        done.stderr
    } else {
        done.stdout
    };
    Ok((!bytes.is_empty()).then_some(bytes))
}

/// 在后台线程中读取管道。超过 `cap` 的部分读后丢弃，保证子进程不会因管道写满而阻塞。
fn drain<R: Read + Send + 'static>(pipe: Option<R>, cap: u64) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        let _ = (&mut pipe).take(cap).read_to_end(&mut buf);
        let _ = io::copy(&mut pipe, &mut io::sink());
        buf
    })
}

/// 杀死进程组 `pgid`。进程组可能已经不存在，错误忽略。
///
/// 通过 `kill -KILL -- -<pgid>` 发送信号，不引入 libc 依赖。
fn kill_group(pgid: u32) {
    let target = format!("-{pgid}");
    let _ = Command::new("kill")
        .args(["-KILL", "--", &target])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{Finished, ProcError, run_captured, run_finished};

    const SH: &str = "/bin/sh";

    fn sh(script: &str, timeout: Duration) -> Result<Finished, ProcError> {
        run_finished(Path::new(SH), &["-c", script], &[], timeout, 1024)
    }

    /// 轮询 `kill -0 pid`，最多等 3 秒，直到进程消失。返回 `true` 表示已消失。
    fn wait_gone(pid: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let alive = Command::new("kill")
                .args(["-0", pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn returns_code_stdout_and_stderr() {
        let done = sh("printf out; printf err >&2; exit 4", Duration::from_secs(5)).unwrap();
        assert_eq!(done.code, Some(4));
        assert_eq!(done.stdout, b"out");
        assert_eq!(done.stderr, b"err");
    }

    #[test]
    fn slow_process_times_out_with_its_limit() {
        let err = sh("sleep 30", Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, ProcError::Timeout(200)), "got {err:?}");
    }

    #[test]
    fn missing_program_is_io_error() {
        let err = run_finished(
            Path::new("/nonexistent/whatcmd-no-such-bin"),
            &[],
            &[],
            Duration::from_secs(1),
            1024,
        )
        .unwrap_err();
        assert!(matches!(err, ProcError::Io(_)), "got {err:?}");
    }

    #[test]
    fn env_pairs_reach_the_child() {
        let done = run_finished(
            Path::new(SH),
            &["-c", "printf '%s' \"$WHATCMD_PROC_TEST\""],
            &[("WHATCMD_PROC_TEST", "yes")],
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(done.stdout, b"yes");
    }

    #[test]
    fn output_is_read_only_up_to_the_cap() {
        // max_bytes = 10，原始读取上限为 10 * 4 = 40 字节。
        let done = run_finished(
            Path::new(SH),
            &["-c", "head -c 1000 /dev/zero"],
            &[],
            Duration::from_secs(5),
            10,
        )
        .unwrap();
        assert_eq!(done.code, Some(0));
        assert_eq!(done.stdout.len(), 40);
    }

    #[test]
    fn captured_output_is_none_when_both_streams_are_empty() {
        let out = run_captured(
            Path::new(SH),
            &["-c", "exit 0"],
            &[],
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn captured_output_falls_back_to_stderr() {
        let out = run_captured(
            Path::new(SH),
            &["-c", "printf e >&2"],
            &[],
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(out, Some(b"e".to_vec()));
    }

    #[test]
    fn normal_exit_kills_leftover_background_process() {
        // 后台 sleep 与 sh 同属一个进程组。sh 正常退出后，它应被进程组清理杀死。
        // pid 通过 stdout 返回，不依赖文件写入时序。
        let done = sh("sleep 30 & echo $!", Duration::from_secs(5)).unwrap();
        assert_eq!(done.code, Some(0));
        let pid = String::from_utf8(done.stdout).unwrap();
        let pid = pid.trim();
        assert!(!pid.is_empty(), "background pid was not printed");
        assert!(
            wait_gone(pid),
            "background process {pid} survived group kill"
        );
    }
}
