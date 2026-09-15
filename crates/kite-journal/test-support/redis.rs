use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};
pub struct TestRedis {
    pub url: String,
    child: Child,
    _dir: tempfile::TempDir,
}
impl TestRedis {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let child = Command::new("redis-server")
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--save",
                "",
                "--appendonly",
                "yes",
                "--appendfsync",
                "always",
                "--maxmemory-policy",
                "noeviction",
                "--dir",
            ])
            .arg(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Redis 7.2+ required for integration tests");
        let result = Self {
            url: format!("redis://127.0.0.1:{port}/0"),
            child,
            _dir: dir,
        };
        for _ in 0..100 {
            if redis::Client::open(result.url.as_str())
                .unwrap()
                .get_connection()
                .is_ok()
            {
                return result;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("Isolated Redis did not start")
    }
    #[allow(dead_code)]
    pub fn namespace(&self) -> String {
        "test".into()
    }
    #[allow(dead_code)]
    pub fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        let port = self
            .url
            .split(':')
            .next_back()
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        self.child = Command::new("redis-server")
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                port,
                "--save",
                "",
                "--appendonly",
                "yes",
                "--appendfsync",
                "always",
                "--maxmemory-policy",
                "noeviction",
                "--dir",
            ])
            .arg(self._dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if redis::Client::open(self.url.as_str())
                .unwrap()
                .get_connection()
                .is_ok()
            {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("Redis restart failed")
    }
    #[allow(dead_code)]
    pub fn connection(&self) -> redis::Connection {
        redis::Client::open(self.url.as_str())
            .unwrap()
            .get_connection()
            .unwrap()
    }
}
impl Drop for TestRedis {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
