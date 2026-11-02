//! Local Feishu credentials. Never include this directory in database backups.
use radar_core::{Result, feishu::Credentials};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "feishu-credentials.json";
const MAX_BYTES: u64 = 8192;

fn directory(root: &Path, create: bool) -> Result<PathBuf> {
    let directory = root.join("private");
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("无法创建飞书本机私有配置目录".into()),
        }
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|_| "请先在设置中保存飞书本机配置")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("飞书私有配置目录无效".into());
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护飞书配置目录")?;
    Ok(directory)
}

fn checked_file(path: &Path) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("飞书本机配置文件无法读取".into()),
    };
    let metadata = file.metadata().map_err(|_| "飞书配置文件无效")?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_BYTES {
        return Err("飞书本机配置文件无效或过大".into());
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| "无法保护飞书配置文件")?;
    Ok(Some(file))
}

pub fn load(root: &Path) -> Result<Credentials> {
    let directory = directory(root, false)?;
    let file = checked_file(&directory.join(FILE_NAME))?.ok_or("请先在设置中保存飞书本机配置")?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "飞书本机配置读取失败")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("飞书本机配置过大".into());
    }
    let credentials: Credentials =
        serde_json::from_slice(&bytes).map_err(|_| "飞书本机配置无效，请重新保存")?;
    credentials.validate()?;
    Ok(credentials)
}

pub fn save(root: &Path, credentials: &Credentials) -> Result<()> {
    credentials.validate()?;
    let directory = directory(root, true)?;
    let destination = directory.join(FILE_NAME);
    // Reject links/special files instead of overwriting an unexpected destination.
    let _ = checked_file(&destination)?;
    let temporary = directory.join(format!(".feishu-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|_| "无法写入飞书本机配置")?;
        let bytes = serde_json::to_vec(credentials).map_err(|_| "飞书配置编码失败")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "飞书本机配置保存失败")?;
        fs::rename(&temporary, &destination).map_err(|_| "飞书本机配置替换失败")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn remove(root: &Path) -> Result<()> {
    if !root
        .join("private")
        .try_exists()
        .map_err(|_| "飞书配置目录无法检查")?
    {
        return Ok(());
    }
    let directory = directory(root, false)?;
    let path = directory.join(FILE_NAME);
    if checked_file(&path)?.is_some() {
        fs::remove_file(path).map_err(|_| "飞书已停用，但本机配置移除失败")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn credentials() -> Credentials {
        Credentials {
            webhook: "https://open.feishu.cn/open-apis/bot/v2/hook/OFFLINE_TEST".into(),
            secret: Some("OFFLINE_TEST_SECRET".into()),
        }
    }

    #[test]
    fn saved_configuration_is_private_and_reusable_without_keychain_across_restarts() {
        let root = tempfile::tempdir().unwrap();
        assert!(load(root.path()).is_err());
        save(root.path(), &credentials()).unwrap();
        for _ in 0..5 {
            let reloaded = load(root.path()).unwrap();
            assert_eq!(reloaded.webhook, credentials().webhook);
            assert_eq!(reloaded.secret, credentials().secret);
        }
        assert_eq!(
            fs::metadata(root.path().join("private"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("private").join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let replacement = Credentials {
            secret: None,
            ..credentials()
        };
        save(root.path(), &replacement).unwrap();
        assert!(load(root.path()).unwrap().secret.is_none());
        assert_eq!(
            fs::read_dir(root.path().join("private")).unwrap().count(),
            1
        );
        remove(root.path()).unwrap();
        remove(root.path()).unwrap();
        assert!(load(root.path()).is_err());
    }

    #[test]
    fn invalid_save_keeps_previous_configuration_and_errors_never_echo_secrets() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &credentials()).unwrap();
        let bad = Credentials {
            webhook: "https://invalid.example/PRIVATE_TEST_SECRET".into(),
            secret: None,
        };
        assert!(
            !save(root.path(), &bad)
                .unwrap_err()
                .contains("PRIVATE_TEST_SECRET")
        );
        assert_eq!(load(root.path()).unwrap().webhook, credentials().webhook);
        fs::write(
            root.path().join("private").join(FILE_NAME),
            b"BROKEN_PRIVATE_TEST_SECRET",
        )
        .unwrap();
        assert!(
            !load(root.path())
                .err()
                .unwrap()
                .contains("PRIVATE_TEST_SECRET")
        );
    }

    #[test]
    fn links_special_files_and_oversized_files_are_not_followed_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("private")).unwrap();
        assert!(save(root.path(), &credentials()).is_err());
        fs::remove_file(root.path().join("private")).unwrap();
        save(root.path(), &credentials()).unwrap();
        let path = root.path().join("private").join(FILE_NAME);
        fs::remove_file(&path).unwrap();
        let outside_file = outside.path().join("keep");
        fs::write(&outside_file, b"keep").unwrap();
        symlink(&outside_file, &path).unwrap();
        assert!(load(root.path()).is_err());
        assert!(save(root.path(), &credentials()).is_err());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep");
        fs::remove_file(&path).unwrap();
        fs::hard_link(&outside_file, &path).unwrap();
        assert!(load(root.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load(root.path()).is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(load(root.path()).is_err());
    }

    #[test]
    fn database_exports_and_backups_do_not_include_private_configuration() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &credentials()).unwrap();
        let db = radar_core::Database::open(&root.path().join("radar.sqlite")).unwrap();
        let backup = root.path().join("backup.sqlite");
        db.backup(&backup).unwrap();
        for bytes in [
            fs::read(backup).unwrap(),
            db.export(radar_core::Source::Manual, "json")
                .unwrap()
                .into_bytes(),
        ] {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("OFFLINE_TEST"));
        }
    }
}
