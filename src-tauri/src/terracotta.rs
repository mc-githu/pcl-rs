//! 陶瓦联机（Terracotta）集成。
//!
//! Terracotta（<https://github.com/burningtnt/Terracotta>，AGPL-3.0）是一个独立的
//! Minecraft 局域网联机工具。PCL-RS **不重新分发**它的代码或二进制，只做三件事：
//!
//! 1. 检测本机是否已安装；
//! 2. 从 Terracotta 官方 GitHub Release 下载 Windows 打包产物并解压到游戏目录下的
//!    `pcl-rs/tools/terracotta/`；
//! 3. 启动/停止它（房间的创建与加入在 Terracotta 自己的界面里完成）。
//!
//! 这样既遵守 AGPL-3.0 的边界，又让用户不必手动去 Releases 页面找文件。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use pcl_net::{HttpClient, SourcePool};

/// Terracotta 官方仓库。
pub const REPO: &str = "burningtnt/Terracotta";
/// 本项目读取的最新版本号接口。
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/burningtnt/Terracotta/releases/latest";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerracottaStatus {
    pub installed: bool,
    pub exe: Option<String>,
    pub version: Option<String>,
    pub running: bool,
    pub dir: String,
}

/// 下载并安装的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerracottaInstall {
    pub version: String,
    pub asset: String,
    pub exe: String,
    pub status: TerracottaStatus,
}

/// 运行中的 Terracotta 进程。
#[derive(Default)]
pub struct TerracottaState {
    child: Mutex<Option<Child>>,
}

impl TerracottaState {
    pub fn running(&self) -> bool {
        let Ok(mut guard) = self.child.lock() else {
            return false;
        };
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                // Still running.
                Ok(None) => true,
                // Exited: drop the handle.
                _ => {
                    *guard = None;
                    false
                }
            },
            None => false,
        }
    }

    pub fn stop(&self) -> Result<()> {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

/// `terracotta.exe` 的存放目录：`<game>/pcl-rs/tools/terracotta/`
pub fn install_dir(game_dir: &Path) -> PathBuf {
    game_dir.join("pcl-rs").join("tools").join("terracotta")
}

/// 在安装目录里找可执行文件（打包结构可能多一层目录）。
pub fn find_exe(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut depth = 0;
    while let Some(current) = stack.pop() {
        if depth > 3 {
            break;
        }
        depth += 1;
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .map(|n| n.eq_ignore_ascii_case("terracotta.exe"))
                .unwrap_or(false)
            {
                return Some(path);
            }
        }
    }
    None
}

fn version_file(dir: &Path) -> PathBuf {
    dir.join("version.txt")
}

pub fn status(game_dir: &Path) -> TerracottaStatus {
    let dir = install_dir(game_dir);
    let exe = find_exe(&dir);
    let version = std::fs::read_to_string(version_file(&dir))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    TerracottaStatus {
        installed: exe.is_some(),
        exe: exe.map(|p| p.display().to_string()),
        version,
        running: false,
        dir: dir.display().to_string(),
    }
}

/// 安装（或更新）Terracotta 到游戏目录。
pub async fn install<F>(game_dir: &Path, mut progress: F) -> Result<TerracottaInstall>
where
    F: FnMut(&str),
{
    progress("正在查询最新版本…");
    let client = HttpClient::new(SourcePool::official())?;

    let release: serde_json::Value = client
        .get_json_with_headers(LATEST_RELEASE_API, &[("accept", "application/vnd.github+json")])
        .await
        .context("查询 Terracotta 最新版本")?;

    let version = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .trim_start_matches('v')
        .to_string();

    let assets = release
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("GitHub 返回的 release 没有 assets"))?;

    // Windows x64 的打包产物：terracotta-<ver>-windows-x86_64-pkg.tar.gz
    let asset = assets
        .iter()
        .find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.contains("windows") && n.contains("x86_64") && n.ends_with(".tar.gz"))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("该版本没有 Windows x64 产物，请到 {REPO}/releases 手动下载"))?;

    let asset_name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("terracotta.tar.gz");
    let url = asset
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| anyhow!("asset 缺少下载地址"))?;

    progress(&format!("正在下载 {asset_name}…"));
    let bytes = client
        .get_bytes(url)
        .await
        .with_context(|| format!("下载 {asset_name}"))?;

    let dir = install_dir(game_dir);
    tokio::fs::create_dir_all(&dir).await?;

    progress("正在解压…");
    let dir_clone = dir.clone();
    tokio::task::spawn_blocking(move || extract_tar_gz(&bytes, &dir_clone))
        .await
        .context("解压任务失败")??;

    let exe = find_exe(&dir).ok_or_else(|| anyhow!("解压后没有找到 terracotta.exe"))?;
    std::fs::write(version_file(&dir), &version)?;

    Ok(TerracottaInstall {
        version,
        asset: asset_name.to_string(),
        exe: exe.display().to_string(),
        status: status(game_dir),
    })
}

/// 解压 tar.gz 到目标目录（带一层目录剥离：打包时通常多包一层）。
fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().context("读取 tar 条目")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();

        // 剥掉第一层目录，避免出现 tools/terracotta/<pkg-name>/...
        let stripped: PathBuf = {
            let mut components = path.components();
            components.next();
            components.as_path().to_path_buf()
        };
        let relative = if stripped.as_os_str().is_empty() {
            path.clone()
        } else {
            stripped
        };

        let out = dest.join(&relative);
        if !out.starts_with(dest) {
            bail!("拒绝解压越界路径: {}", out.display());
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf).with_context(|| format!("写入 {}", out.display()))?;
    }
    Ok(())
}

/// 启动 Terracotta（工作目录设为游戏目录，方便它自己扫描 `.minecraft`）。
pub fn launch(game_dir: &Path, exe: &Path, state: &TerracottaState) -> Result<u32> {
    if state.running() {
        bail!("陶瓦联机已经在运行了");
    }
    let child = Command::new(exe)
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("启动 {}", exe.display()))?;
    let pid = child.id();
    if let Ok(mut guard) = state.child.lock() {
        *guard = Some(child);
    }
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_is_under_game_dir() {
        let dir = install_dir(Path::new("C:/mc"));
        assert!(dir.ends_with(Path::new("pcl-rs").join("tools").join("terracotta")));
    }

    #[test]
    fn finds_exe_in_nested_package() {
        let root = std::env::temp_dir().join("pcl-rs-terracotta-test");
        let _ = std::fs::remove_dir_all(&root);
        let nested = root.join("pkg-0.4.2");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("terracotta.exe"), b"x").unwrap();

        let found = find_exe(&root).expect("should find exe");
        assert!(found.ends_with("terracotta.exe"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
