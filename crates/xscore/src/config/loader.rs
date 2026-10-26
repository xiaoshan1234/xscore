// config/loader.rs
use crate::core::Result;
use crate::model::ConnectSpec;
use std::path::Path;

pub fn load_connect_spec(path: &Path) -> Result<ConnectSpec> {
    let bytes = std::fs::read(path)
        .map_err(|e| crate::core::Error::Config(format!("read {}: {e}", path.display())))?;
    let spec: ConnectSpec = serde_json::from_slice(&bytes)
        .map_err(|e| crate::core::Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(spec)
}

pub fn save_connect_spec(path: &Path, spec: &ConnectSpec) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::core::Error::Config(format!("mkdir {}: {e}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(spec)?;
    std::fs::write(path, bytes)
        .map_err(|e| crate::core::Error::Config(format!("write {}: {e}", path.display())))?;
    Ok(())
}

pub fn list_link_configs() -> Result<Vec<std::path::PathBuf>> {
    let dir = super::paths::links_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| crate::core::Error::Config(format!("read_dir {}: {e}", dir.display())))?
    {
        let e = entry.map_err(|e| crate::core::Error::Config(format!("dir entry: {e}")))?;
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
