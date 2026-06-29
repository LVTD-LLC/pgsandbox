use std::{
    fs,
    io::ErrorKind,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const LOCAL_PROFILE_NAME: &str = "local";

const ADMIN_USER: &str = "pgsandbox_admin";
const CONFIG_FILE_NAME: &str = "local-postgres.json";
const DATA_DIR: &str = "postgres/data";
const LOCK_FILE_NAME: &str = "local-postgres.lock";
const LOG_FILE: &str = "postgres/postgres.log";
const PASSWORD_FILE: &str = "postgres/initdb-password";
const SOCKET_DIR: &str = "postgres/run";
const DEFAULT_LOCAL_PORT: u16 = 65432;
const REQUIRED_LOCAL_BINARIES: &[&str] = &["initdb", "pg_ctl", "postgres"];
const POSTGRES_CONF_BEGIN: &str = "# BEGIN PGSandbox local runtime";
const POSTGRES_CONF_END: &str = "# END PGSandbox local runtime";

#[derive(Clone, Debug)]
pub struct LocalPostgresCluster {
    root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalClusterConfig {
    pub profile_name: String,
    pub admin_url: String,
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub socket_dir: PathBuf,
    pub log_file: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalClusterStatus {
    pub initialized: bool,
    pub running: bool,
    pub config: Option<LocalClusterConfig>,
}

struct LocalClusterLock {
    _file: fs::File,
}

impl LocalPostgresCluster {
    pub fn from_env() -> anyhow::Result<Self> {
        let root = match std::env::var_os("PGSANDBOX_HOME") {
            Some(path) => PathBuf::from(path),
            None => dirs::home_dir()
                .context("could not resolve home directory for ~/.pgsandbox")?
                .join(".pgsandbox"),
        };
        Ok(Self::new(root))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE_NAME)
    }

    pub fn ensure_started(&self) -> anyhow::Result<LocalClusterConfig> {
        let _lock = self.acquire_lock()?;
        self.ensure_started_locked()
    }

    fn ensure_started_locked(&self) -> anyhow::Result<LocalClusterConfig> {
        let initialized = self.init_locked()?;
        if self.is_running()? {
            return Ok(initialized);
        }
        self.start_locked()
    }

    pub fn init(&self) -> anyhow::Result<LocalClusterConfig> {
        let _lock = self.acquire_lock()?;
        self.init_locked()
    }

    fn init_locked(&self) -> anyhow::Result<LocalClusterConfig> {
        if self.data_dir().join("PG_VERSION").exists() {
            return self.read_config();
        }

        ensure_required_local_binaries()?;
        fs::create_dir_all(self.socket_dir()).with_context(|| {
            format!(
                "failed to create local Postgres socket directory {}",
                self.socket_dir().display()
            )
        })?;

        let password = local_admin_password();
        let mut config = self.config_for_port(select_free_port()?, &password);
        write_password_file(&self.password_file(), &password)?;

        let init_result = Command::new("initdb")
            .arg("-D")
            .arg(self.data_dir())
            .arg("--username")
            .arg(ADMIN_USER)
            .arg("--pwfile")
            .arg(self.password_file())
            .arg("--auth-host")
            .arg("scram-sha-256")
            .arg("--auth-local")
            .arg("trust")
            .arg("--encoding")
            .arg("UTF8")
            .output();

        let _ = fs::remove_file(self.password_file());
        command_output("initdb", init_result)?;

        fs::create_dir_all(&config.socket_dir).with_context(|| {
            format!(
                "failed to create local Postgres socket directory {}",
                config.socket_dir.display()
            )
        })?;
        write_postgres_runtime_config(&config)?;
        self.write_config(&config)?;

        // Re-read from disk so callers receive exactly what the config file records.
        config = self.read_config()?;
        Ok(config)
    }

    pub fn start(&self) -> anyhow::Result<LocalClusterConfig> {
        let _lock = self.acquire_lock()?;
        self.start_locked()
    }

    fn start_locked(&self) -> anyhow::Result<LocalClusterConfig> {
        let mut config = self.init_locked()?;
        if self.is_running()? {
            return Ok(config);
        }

        let original_config = config.clone();
        config = start_config_for_available_port(config, port_available, select_free_port)?;
        ensure_postgres_binary("pg_ctl")?;
        fs::create_dir_all(&config.socket_dir).with_context(|| {
            format!(
                "failed to create local Postgres socket directory {}",
                config.socket_dir.display()
            )
        })?;
        write_postgres_runtime_config(&config)?;
        if let Err(error) = self.write_config(&config) {
            return Err(self.restore_start_config_after_error(&original_config, error));
        }

        if let Err(error) = command_status(
            "pg_ctl",
            Command::new("pg_ctl")
                .arg("-D")
                .arg(&config.data_dir)
                .arg("-l")
                .arg(&config.log_file)
                .arg("-w")
                .arg("start")
                .status(),
        ) {
            return Err(self.restore_start_config_after_error(&original_config, error));
        }

        if !self.is_running()? {
            return Err(self.restore_start_config_after_error(
                &original_config,
                anyhow::anyhow!("local Postgres did not report healthy after pg_ctl start"),
            ));
        }

        Ok(config)
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        let _lock = self.acquire_lock()?;
        self.stop_locked()
    }

    fn stop_locked(&self) -> anyhow::Result<()> {
        if !self.data_dir().join("PG_VERSION").exists() {
            return Ok(());
        }
        if !self.is_running()? {
            return Ok(());
        }

        ensure_postgres_binary("pg_ctl")?;
        command_status(
            "pg_ctl",
            Command::new("pg_ctl")
                .arg("-D")
                .arg(self.data_dir())
                .arg("-m")
                .arg("fast")
                .arg("-w")
                .arg("stop")
                .status(),
        )
    }

    pub fn status(&self) -> anyhow::Result<LocalClusterStatus> {
        let _lock = self.acquire_lock()?;
        self.status_locked()
    }

    fn status_locked(&self) -> anyhow::Result<LocalClusterStatus> {
        let initialized = self.data_dir().join("PG_VERSION").exists();
        if !initialized {
            return Ok(LocalClusterStatus {
                initialized: false,
                running: false,
                config: None,
            });
        }

        let config = self.read_config()?;
        Ok(LocalClusterStatus {
            initialized: true,
            running: self.is_running()?,
            config: Some(config),
        })
    }

    fn config_for_port(&self, port: u16, password: &str) -> LocalClusterConfig {
        LocalClusterConfig {
            profile_name: LOCAL_PROFILE_NAME.to_string(),
            admin_url: admin_url_for(port, password),
            host: "127.0.0.1".to_string(),
            port,
            data_dir: self.data_dir(),
            socket_dir: self.socket_dir(),
            log_file: self.log_file(),
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    fn log_file(&self) -> PathBuf {
        self.root.join(LOG_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE_NAME)
    }

    fn password_file(&self) -> PathBuf {
        self.root.join(PASSWORD_FILE)
    }

    fn acquire_lock(&self) -> anyhow::Result<LocalClusterLock> {
        fs::create_dir_all(&self.root).with_context(|| {
            format!(
                "failed to create local Postgres root directory {}",
                self.root.display()
            )
        })?;
        let path = self.lock_path();
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open local Postgres lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("failed to lock local Postgres state {}", path.display()))?;
        Ok(LocalClusterLock { _file: file })
    }

    fn read_config(&self) -> anyhow::Result<LocalClusterConfig> {
        let path = self.config_path();
        let raw = fs::read_to_string(&path).with_context(|| {
            format!(
                "local Postgres data dir exists but config file is missing or unreadable at {}",
                path.display()
            )
        })?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "local Postgres config file is not valid JSON at {}",
                path.display()
            )
        })
    }

    fn socket_dir(&self) -> PathBuf {
        self.root.join(SOCKET_DIR)
    }

    fn write_config(&self, config: &LocalClusterConfig) -> anyhow::Result<()> {
        let path = self.config_path();
        write_private_file_atomically(
            &path,
            &format!("{}\n", serde_json::to_string_pretty(config)?),
        )
        .with_context(|| format!("failed to write local Postgres config {}", path.display()))
    }

    fn restore_start_config(&self, config: &LocalClusterConfig) -> anyhow::Result<()> {
        self.write_config(config)?;
        write_postgres_runtime_config(config)
    }

    fn restore_start_config_after_error(
        &self,
        config: &LocalClusterConfig,
        original_error: anyhow::Error,
    ) -> anyhow::Error {
        match self.restore_start_config(config) {
            Ok(()) => original_error,
            Err(rollback_error) => anyhow::anyhow!(
                "{original_error:#}; additionally failed to restore previous local Postgres config: {rollback_error:#}"
            ),
        }
    }

    fn is_running(&self) -> anyhow::Result<bool> {
        if !self.data_dir().join("PG_VERSION").exists() {
            return Ok(false);
        }

        ensure_postgres_binary("pg_ctl")?;
        match Command::new("pg_ctl")
            .arg("-D")
            .arg(self.data_dir())
            .arg("status")
            .status()
        {
            Ok(status) => Ok(status.success()),
            Err(error) if error.kind() == ErrorKind::NotFound => missing_binary("pg_ctl"),
            Err(error) => Err(error).context("failed to run pg_ctl status"),
        }
    }
}

pub(crate) fn select_free_port_with_probe<F>(mut available: F) -> Option<u16>
where
    F: FnMut(u16) -> bool,
{
    (DEFAULT_LOCAL_PORT..=u16::MAX).find(|port| available(*port))
}

fn required_local_binaries() -> &'static [&'static str] {
    REQUIRED_LOCAL_BINARIES
}

fn ensure_required_local_binaries() -> anyhow::Result<()> {
    for binary in required_local_binaries() {
        ensure_postgres_binary(binary)?;
    }
    Ok(())
}

fn select_free_port() -> anyhow::Result<u16> {
    select_free_port_with_probe(port_available).context("could not find a free high local port")
}

fn port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn start_config_for_available_port<F, S>(
    mut config: LocalClusterConfig,
    available: F,
    select: S,
) -> anyhow::Result<LocalClusterConfig>
where
    F: FnOnce(u16) -> bool,
    S: FnOnce() -> anyhow::Result<u16>,
{
    if available(config.port) {
        return Ok(config);
    }

    config.port = select()?;
    config.admin_url = admin_url_for(config.port, &admin_password_from_url(&config.admin_url)?);
    Ok(config)
}

fn admin_password_from_url(admin_url: &str) -> anyhow::Result<String> {
    let parsed = url::Url::parse(admin_url).context("stored local admin URL is invalid")?;
    parsed
        .password()
        .map(ToOwned::to_owned)
        .context("stored local admin URL is missing its password")
}

fn admin_url_for(port: u16, password: &str) -> String {
    format!("postgres://{ADMIN_USER}:{password}@127.0.0.1:{port}/postgres?sslmode=disable")
}

fn local_admin_password() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn ensure_postgres_binary(binary: &'static str) -> anyhow::Result<()> {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => anyhow::bail!(
            "Postgres binary `{binary}` is installed but failed to run: {}",
            summarize_stderr(&output.stderr)
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => missing_binary(binary),
        Err(error) => Err(error).with_context(|| format!("failed to run `{binary} --version`")),
    }
}

fn missing_binary<T>(binary: &'static str) -> anyhow::Result<T> {
    anyhow::bail!(
        "Postgres binary `{binary}` was not found on PATH. Install PostgreSQL locally so PGSandbox can manage ~/.pgsandbox/postgres without using Docker."
    )
}

fn command_output(
    binary: &'static str,
    result: std::io::Result<std::process::Output>,
) -> anyhow::Result<()> {
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => failed_command(binary, output.status, &output.stderr),
        Err(error) if error.kind() == ErrorKind::NotFound => missing_binary(binary),
        Err(error) => Err(error).with_context(|| format!("failed to start `{binary}`")),
    }
}

fn command_status(binary: &'static str, result: std::io::Result<ExitStatus>) -> anyhow::Result<()> {
    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => failed_command(binary, status, &[]),
        Err(error) if error.kind() == ErrorKind::NotFound => missing_binary(binary),
        Err(error) => Err(error).with_context(|| format!("failed to start `{binary}`")),
    }
}

fn failed_command(binary: &str, status: ExitStatus, stderr: &[u8]) -> anyhow::Result<()> {
    anyhow::bail!(
        "`{binary}` failed with status {}: {}",
        status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated by signal".to_string()),
        summarize_stderr(stderr)
    )
}

fn summarize_stderr(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_string();
    if message.is_empty() {
        "(no stderr)".to_string()
    } else {
        message
    }
}

fn write_password_file(path: &Path, password: &str) -> anyhow::Result<()> {
    write_private_file(path, &format!("{password}\n"))
}

fn write_private_file_atomically(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create private local Postgres file directory {}",
                parent.display()
            )
        })?;
    }

    let temporary_path = temporary_private_file_path(path);
    let result = write_private_file(&temporary_path, content).and_then(|()| {
        fs::rename(&temporary_path, path).with_context(|| {
            format!(
                "failed to replace private local Postgres file {}",
                path.display()
            )
        })
    });

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }

    result
}

fn temporary_private_file_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "local-postgres".into());
    path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()))
}

fn write_private_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create private local Postgres file directory {}",
                parent.display()
            )
        })?;
    }

    #[cfg(unix)]
    {
        use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| {
                format!(
                    "failed to write private local Postgres file {}",
                    path.display()
                )
            })?;
        file.write_all(content.as_bytes())?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, content).with_context(|| {
            format!(
                "failed to write private local Postgres file {}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn write_postgres_runtime_config(config: &LocalClusterConfig) -> anyhow::Result<()> {
    let path = config.data_dir.join("postgresql.conf");
    let existing = fs::read_to_string(&path)
        .with_context(|| format!("failed to read local Postgres config {}", path.display()))?;
    let block = postgres_runtime_config_block(config);
    fs::write(&path, replace_managed_block(&existing, &block))
        .with_context(|| format!("failed to write local Postgres config {}", path.display()))
}

fn postgres_runtime_config_block(config: &LocalClusterConfig) -> String {
    format!(
        "{POSTGRES_CONF_BEGIN}\nlisten_addresses = {}\nport = {}\nunix_socket_directories = {}\n{POSTGRES_CONF_END}\n",
        postgres_conf_literal(&config.host),
        config.port,
        postgres_conf_literal(&config.socket_dir.to_string_lossy()),
    )
}

fn replace_managed_block(existing: &str, block: &str) -> String {
    let Some(start) = existing.find(POSTGRES_CONF_BEGIN) else {
        return format!("{}\n{}", existing.trim_end(), block);
    };
    let Some(relative_end) = existing[start..].find(POSTGRES_CONF_END) else {
        return format!("{}\n{}", existing.trim_end(), block);
    };
    let end = start + relative_end + POSTGRES_CONF_END.len();
    format!(
        "{}{}{}",
        &existing[..start],
        block.trim_end(),
        &existing[end..]
    )
}

fn postgres_conf_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_selection_starts_above_common_postgres_port() {
        let mut checked = Vec::new();
        let port = select_free_port_with_probe(|candidate| {
            checked.push(candidate);
            candidate != 5432
        })
        .unwrap();

        assert_eq!(port, 65432);
        assert!(!checked.contains(&5432));
    }

    #[test]
    fn port_selection_skips_occupied_high_ports() {
        let port = select_free_port_with_probe(|candidate| candidate != 65432).unwrap();

        assert_eq!(port, 65433);
    }

    #[test]
    fn required_local_binaries_include_postgres_server() {
        let binaries = required_local_binaries();

        assert!(binaries.contains(&"initdb"));
        assert!(binaries.contains(&"pg_ctl"));
        assert!(binaries.contains(&"postgres"));
    }

    #[test]
    fn persisted_config_documents_local_runtime_paths() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let config = cluster.config_for_port(65432, "secret");

        cluster.write_config(&config).unwrap();

        let saved = cluster.read_config().unwrap();
        assert_eq!(saved.port, 65432);
        assert_eq!(saved.data_dir, directory.path().join(DATA_DIR));
        assert_eq!(saved.socket_dir, directory.path().join(SOCKET_DIR));
        assert_eq!(saved.admin_url, config.admin_url);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_config_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let config = cluster.config_for_port(65432, "secret");

        cluster.write_config(&config).unwrap();

        let mode = fs::metadata(cluster.config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn local_cluster_lock_is_exclusive() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let _lock = cluster.acquire_lock().unwrap();
        let second = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(cluster.lock_path())
            .unwrap();

        assert!(second.try_lock().is_err());
    }

    #[test]
    fn start_config_reuses_saved_port_when_available() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let config = cluster.config_for_port(65432, "secret");

        let updated = start_config_for_available_port(
            config.clone(),
            |_| true,
            || anyhow::bail!("should not select another port"),
        )
        .unwrap();

        assert_eq!(updated, config);
    }

    #[test]
    fn start_config_updates_admin_url_when_saved_port_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let config = cluster.config_for_port(65432, "secret");

        let updated = start_config_for_available_port(config, |_| false, || Ok(65433)).unwrap();

        assert_eq!(updated.port, 65433);
        assert_eq!(
            updated.admin_url,
            "postgres://pgsandbox_admin:secret@127.0.0.1:65433/postgres?sslmode=disable"
        );
    }

    #[test]
    fn restore_start_config_reverts_runtime_config_and_saved_json() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        fs::create_dir_all(cluster.data_dir()).unwrap();
        fs::write(
            cluster.data_dir().join("postgresql.conf"),
            "# base config\n",
        )
        .unwrap();

        let original = cluster.config_for_port(65432, "secret");
        let updated = cluster.config_for_port(65433, "secret");
        write_postgres_runtime_config(&updated).unwrap();
        cluster.write_config(&updated).unwrap();

        cluster.restore_start_config(&original).unwrap();

        assert_eq!(cluster.read_config().unwrap(), original);
        let postgres_conf = fs::read_to_string(cluster.data_dir().join("postgresql.conf")).unwrap();
        assert!(postgres_conf.contains("port = 65432"));
        assert!(!postgres_conf.contains("port = 65433"));
    }

    #[test]
    fn restore_start_config_reports_saved_config_failures_without_rewriting_runtime_config() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        fs::create_dir_all(cluster.data_dir()).unwrap();

        let original = cluster.config_for_port(65432, "secret");
        let updated = cluster.config_for_port(65433, "secret");
        fs::create_dir(cluster.config_path()).unwrap();
        fs::write(
            cluster.data_dir().join("postgresql.conf"),
            postgres_runtime_config_block(&updated),
        )
        .unwrap();

        let error = cluster.restore_start_config(&original).unwrap_err();

        assert!(error
            .to_string()
            .contains("failed to write local Postgres config"));
        let postgres_conf = fs::read_to_string(cluster.data_dir().join("postgresql.conf")).unwrap();
        assert!(postgres_conf.contains("port = 65433"));
        assert!(!postgres_conf.contains("port = 65432"));
    }

    #[test]
    fn atomic_config_write_replaces_previous_config() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new(directory.path());
        let original = cluster.config_for_port(65432, "secret");
        let updated = cluster.config_for_port(65433, "secret");

        cluster.write_config(&original).unwrap();
        cluster.write_config(&updated).unwrap();

        assert_eq!(cluster.read_config().unwrap(), updated);
        let leftovers = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }
}
