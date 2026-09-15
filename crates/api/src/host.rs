//! 宿主进程管理：spawn、stderr 尾部采集、终止与异步收尸，以及跨线程
//! 外部控制柄（「终止按钮」路径，docs/architecture/03：不排队在 Game
//! 请求之后）。从 harness 拆出——进程生命周期与会话语义无耦合。

use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::harness::SessionConfig;

// ---------------------------------------------------------------------------
// 终止按钮独立控制路径（docs/architecture/03：不排队在 Game 请求之后）
// ---------------------------------------------------------------------------

/// 宿主进程的外部控制柄：从任意线程直接终止宿主，不经过世界线程的
/// Game 请求队列。置位 killed 后杀进程，世界线程在消息循环出口按
/// `killed_by_us` 将故障分类为 `KILLED_BY_MAIN`。
#[derive(Debug, Clone, Default)]
pub struct HostControl {
    pub(crate) child: Option<Arc<Mutex<Child>>>,
    pub(crate) killed: Arc<AtomicBool>,
}

impl HostControl {
    /// 标记为主进程主动终止并杀死宿主（幂等；宿主不在场则仅置位标记，
    /// 下一次执行的写失败 / EOF 路径据此分类）。
    pub fn kill(&self) {
        // 无宿主在场（尚未加载代码 / 刚被回收）时不置标记：跨代次残留的
        // 标记会把之后真实 crash 误分类为 KILLED_BY_MAIN。
        let Some(child) = &self.child else {
            return;
        };
        self.killed.store(true, Ordering::SeqCst);
        kill_and_reap(child);
    }

    /// 是否已被标记为主进程主动终止。
    pub fn is_marked(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// 宿主进程
// ---------------------------------------------------------------------------

pub(crate) struct HostProc {
    pub(crate) child: Arc<Mutex<Child>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    /// 执行期读到 EOF / 坏帧，或被本进程终止（进程退出）。
    pub(crate) dead: Arc<AtomicBool>,
    #[allow(dead_code)]
    stderr_thread: JoinHandle<()>,
}

impl HostProc {
    pub(crate) fn spawn(
        cfg: &SessionConfig,
    ) -> std::io::Result<(
        HostProc,
        ChildStdin,
        std::io::BufReader<std::process::ChildStdout>,
    )> {
        let mut cmd = Command::new(&cfg.host_bin);
        cmd.args([
            "--heap-limit",
            &cfg.heap_limit.to_string(),
            "--stack-limit",
            &cfg.stack_limit.to_string(),
            "--frame-limit",
            &cfg.frame_limit.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let dead = Arc::new(AtomicBool::new(false));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let tail = stderr_tail.clone();
        let stderr_thread = std::thread::Builder::new()
            .name("ztw-host-stderr".into())
            .spawn(move || {
                use std::io::Read;
                let mut stderr = stderr;
                let mut buf = [0u8; 1024];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut t = tail.lock().unwrap();
                            let keep = 16 * 1024;
                            if t.len() + n > keep {
                                let drop = (t.len() + n - keep).min(t.len());
                                t.drain(0..drop);
                            }
                            t.extend_from_slice(&buf[..n]);
                        }
                    }
                }
            })
            .expect("spawn stderr reader");
        let proc = HostProc {
            child: Arc::new(Mutex::new(child)),
            stderr_tail,
            dead,
            stderr_thread,
        };
        // BufReader：宿主单次写整帧后，前缀 + 体的两次 read 命中同一缓冲，
        // 每帧一次 syscall（跨帧残余也留在缓冲里）。
        Ok((proc, stdin, std::io::BufReader::new(stdout)))
    }

    pub(crate) fn kill(&self) {
        self.dead.store(true, Ordering::SeqCst);
        kill_and_reap(&self.child);
    }

    pub(crate) fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr_tail.lock().unwrap()).into_owned()
    }
}

/// 终止宿主并异步回收：SIGKILL 后由独立线程 wait() 收尸，
/// 避免长时间浸泡运行累积僵尸进程。
pub(crate) fn kill_and_reap(child: &Arc<Mutex<Child>>) {
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
    }
    let child = child.clone();
    let _ = std::thread::Builder::new()
        .name("ztw-reaper".into())
        .spawn(move || {
            if let Ok(mut c) = child.lock() {
                let _ = c.wait();
            }
        });
}
