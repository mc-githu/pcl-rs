//! Native library extraction.
//!
//! LWJGL ships platform natives inside classifier jars; the launcher must unpack
//! them next to the version so `-Djava.library.path` can find `lwjgl.dll`.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// Extract `jar` into `dest`, skipping `excludes` (directory prefixes such as
/// `META-INF/`). Returns the number of files written.
pub fn extract_natives(jar: &Path, dest: &Path, excludes: &[String]) -> Result<usize> {
    let file =
        std::fs::File::open(jar).with_context(|| format!("opening native jar {}", jar.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("reading native jar {}", jar.display()))?;

    std::fs::create_dir_all(dest)
        .with_context(|| format!("creating natives dir {}", dest.display()))?;

    let mut written = 0usize;
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!("skipping bad entry {index} in {}: {e}", jar.display());
                continue;
            }
        };

        let Some(path) = entry.enclosed_name() else {
            tracing::warn!("refusing unsafe zip path in {}", jar.display());
            continue;
        };
        let name = path.to_string_lossy().replace('\\', "/");

        if entry.is_dir() {
            continue;
        }
        if excludes.iter().any(|ex| {
            let ex = ex.trim_end_matches('/');
            name == ex || name.starts_with(&format!("{ex}/"))
        }) {
            continue;
        }

        let out = dest.join(path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .with_context(|| format!("reading {name} from {}", jar.display()))?;
        std::fs::write(&out, &buf).with_context(|| format!("writing {}", out.display()))?;
        written += 1;
    }

    tracing::debug!(
        "extracted {written} natives from {} into {}",
        jar.display(),
        dest.display()
    );
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_and_respects_excludes() {
        let dir = std::env::temp_dir().join("pcl-rs-natives-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jar = dir.join("natives.jar");

        {
            let file = std::fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("lwjgl.dll".to_string(), opts).unwrap();
            zip.write_all(b"binary").unwrap();
            zip.start_file("META-INF/MANIFEST.MF".to_string(), opts).unwrap();
            zip.write_all(b"manifest").unwrap();
            zip.finish().unwrap();
        }

        let out = dir.join("natives");
        let n = extract_natives(&jar, &out, &["META-INF/".to_string()]).unwrap();
        assert_eq!(n, 1);
        assert!(out.join("lwjgl.dll").is_file());
        assert!(!out.join("META-INF").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
