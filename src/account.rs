//! Cuenta de KHInsider. El usuario se guarda en `~/.config/khi-ui/user` y la contraseña, en texto
//! plano, en `~/.config/khi-ui/password`, legible únicamente por el usuario (permisos 0600 en Unix).

use std::path::PathBuf;

/// Usuario y contraseña guardados, si hay una sesión recordada.
pub fn saved() -> Option<(String, String)> {
    let user = crate::settings::get("user").filter(|u| !u.is_empty())?;
    crate::settings::get("password").map(|p| (user, p))
}

/// Guarda el usuario y la contraseña. Devuelve `Err` con el motivo si no se pudo escribir el archivo.
pub fn save(user: &str, password: &str) -> Result<(), String> {
    crate::settings::set("user", user);
    let path = password_file().ok_or("no hay carpeta de configuración")?;
    write_private(&path, password).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn forget() {
    let _ = password_file().map(std::fs::remove_file);
    crate::settings::set("user", "");
}

fn password_file() -> Option<PathBuf> {
    Some(crate::settings::dir()?.join("password"))
}

/// Escribe el archivo con permisos 0600 en Unix: solo el dueño lo lee.
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap_or(path))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(contents.as_bytes())
}

#[cfg(all(test, unix))]
#[test]
fn password_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("khi-ui-password-{}", std::process::id()));
    write_private(&path, "secreto").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!((std::fs::read_to_string(&path).unwrap().as_str(), mode), ("secreto", 0o600));
    std::fs::remove_file(path).unwrap();
}
