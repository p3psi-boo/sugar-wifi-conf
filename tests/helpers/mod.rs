//! 测试辅助模块
//!
//! 提供测试用的通用工具和 Mock 实现

use std::path::PathBuf;
use tempfile::TempDir;

/// 测试上下文，管理临时资源
pub struct TestContext {
    temp_dir: TempDir,
}

impl TestContext {
    /// 创建新的测试上下文
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            temp_dir: TempDir::new()?,
        })
    }

    /// 获取临时目录路径
    pub fn path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    /// 创建临时文件并返回路径
    pub fn create_file(&self, name: &str, content: &str) -> anyhow::Result<PathBuf> {
        let path = self.temp_dir.path().join(name);
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new().expect("Failed to create test context")
    }
}

/// 测试 fixtures 路径
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// 运行异步测试的辅助宏
#[macro_export]
macro_rules! async_test {
    ($test_name:ident, $body:expr) => {
        #[tokio::test]
        async fn $test_name() {
            $body
        }
    };
}
