use std::path::Path;
use std::io;

pub fn atomic_write_secret(path: &Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!("{}.tmp.{}", path.display(), std::process::id());
    let tmp_path = parent.join(&tmp_name);

    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }

    Ok(())
}
