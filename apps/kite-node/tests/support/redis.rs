use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
struct Redis {
    child: Child,
    dir: tempfile::TempDir,
    url: String,
}
impl Redis {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new("redis-server")
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--appendonly",
                "yes",
                "--appendfsync",
                "everysec",
                "--save",
                "",
                "--maxmemory-policy",
                "noeviction",
                "--dir",
                dir.path().to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("redis-server required");
        let server = Self {
            child,
            dir,
            url: format!("redis://127.0.0.1:{port}/"),
        };
        for _ in 0..100 {
            if redis::Client::open(server.url.as_str())
                .unwrap()
                .get_connection()
                .is_ok()
            {
                return server;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Redis did not start");
    }
    fn run(&self, args: &[&str]) -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args(args)
            .env("KITE_REDIS_URL", &self.url)
            .current_dir(self.dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "command {:?}: {}\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!logs.contains("[ERROR]"), "Native runtime error: {logs}");
        assert!(
            !logs.contains("InvalidStateTrigger"),
            "Native event sequence failure: {logs}"
        );
        assert!(
            !logs.contains("on_start` handler was called when not overridden"),
            "Missing lifecycle hook: {logs}"
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok())
            .expect("JSON result")
    }
}
impl Drop for Redis {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
