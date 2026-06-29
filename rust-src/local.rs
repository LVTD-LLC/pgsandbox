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
        let initialized = self.init()?;
        if self.is_running()? {
            return Ok(initialized);
        }
        self.start()
    }

    pub fn init(&self) -> anyhow::Result<LocalClusterConfig> {
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
        let mut config = self.init()?;
        if self.is_running()? {
            return Ok(config);
        }

        config.port = select_free_port()?;
        config.admin_url = admin_url_for(config.port, &admin_password_from_url(&config.admin_url)?);
        fs::create_dir_all(&config.socket_dir).with_context(|| {
            format!(
                "failed to create local Postgres socket directory {}",
                config.socket_dir.display()
            )
        })?;
        write_postgres_runtime_config(&config)?;
        self.write_config(&config)?;

        ensure_postgres_binary("pg_ctl")?;
        command_status(
            "pg_ctl",
            Command::new("pg_ctl")
                .arg("-D")
                .arg(&config.data_dir)
                .arg("-l")
                .arg(&config.log_file)
                .arg("-w")
                .arg("start")
                .status(),
        )?;

        if !self.is_running()? {
            anyhow::bail!("local Postgres did not report healthy after pg_ctl start");
        }

        self.read_config()
    }

    pub fn stop(&self) -> anyhow::Result<()> {
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

    fn password_file(&self) -> PathBuf {
        self.root.join(PASSWORD_FILE)
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
        write_private_file(
            &path,
            &format!("{}\n", serde_json::to_string_pretty(config)?),
        )
        .with_context(|| format!("failed to write local Postgres config {}", path.display()))
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
}
