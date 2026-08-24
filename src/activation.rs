use crate::application::ApplicationIntent;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    Listener as LocalSocketListener, ListenerOptions, Stream as LocalSocketStream, prelude::*,
};
use std::{io, thread};

pub(crate) struct ActivationListener(LocalSocketListener);

impl ActivationListener {
    pub(crate) fn claim_or_forward() -> Result<Option<Self>, String> {
        let name = activation_name()?;
        match listener(name.clone()) {
            Ok(listener) => Ok(Some(Self(listener))),
            Err(listen_error) => {
                for _ in 0..20 {
                    match LocalSocketStream::connect(name.clone()) {
                        Ok(stream) => {
                            drop(stream);
                            return Ok(None);
                        }
                        Err(_) => thread::sleep(std::time::Duration::from_millis(25)),
                    }
                }
                #[cfg(unix)]
                if let Ok(listener) = reclaim_stale_listener(name) {
                    return Ok(Some(Self(listener)));
                }
                Err(format!(
                    "claim application activation endpoint: {listen_error}"
                ))
            }
        }
    }

    pub(crate) fn start(self, intents: flume::Sender<ApplicationIntent>) -> Result<(), String> {
        thread::Builder::new()
            .name("application-activation".into())
            .spawn(move || {
                while self.0.accept().is_ok() {
                    if intents.send(ApplicationIntent::OpenOrFocus).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("spawn application activation listener: {error}"))?;
        Ok(())
    }
}

#[cfg(unix)]
fn reclaim_stale_listener(
    name: interprocess::local_socket::Name<'static>,
) -> io::Result<LocalSocketListener> {
    use interprocess::os::unix::local_socket::ListenerOptionsExt;

    ListenerOptions::new()
        .name(name)
        .mode(0o600)
        .try_overwrite(true)
        .max_spin_time(std::time::Duration::from_millis(100))
        .create_sync()
}

fn listener(name: interprocess::local_socket::Name<'static>) -> io::Result<LocalSocketListener> {
    let options = ListenerOptions::new().name(name);
    #[cfg(unix)]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        options.mode(0o600)
    };
    options.create_sync()
}

#[cfg(windows)]
fn activation_name() -> Result<interprocess::local_socket::Name<'static>, String> {
    let scope = user_scope();
    format!("agent-terminal-{scope}-activation")
        .to_ns_name::<GenericNamespaced>()
        .map(interprocess::local_socket::Name::into_owned)
        .map_err(|error| format!("name application activation endpoint: {error}"))
}

#[cfg(unix)]
fn activation_name() -> Result<interprocess::local_socket::Name<'static>, String> {
    activation_directory()?
        .join("agent-terminal-activation.sock")
        .to_fs_name::<GenericFilePath>()
        .map(interprocess::local_socket::Name::into_owned)
        .map_err(|error| format!("name application activation endpoint: {error}"))
}

#[cfg(unix)]
fn activation_directory() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;

    if let Some(directory) = std::env::var_os("XDG_RUNTIME_DIR") {
        let directory = std::path::PathBuf::from(directory);
        verify_private_directory(&directory)?;
        return Ok(directory);
    }

    let directory = std::env::temp_dir().join(format!("agent-terminal-{}", user_scope()));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "create application activation directory {}: {error}",
                directory.display()
            ));
        }
    }
    verify_private_directory(&directory)?;
    Ok(directory)
}

#[cfg(unix)]
fn verify_private_directory(directory: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "inspect application activation directory {}: {error}",
            directory.display()
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "application activation directory {} is not owned by the current user",
            directory.display()
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "application activation directory {} is accessible to other users",
            directory.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn user_scope() -> String {
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(windows)]
fn user_scope() -> String {
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    format!("{domain}-{user}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect()
}
