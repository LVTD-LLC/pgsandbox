use std::{
    collections::BTreeSet,
    env,
    ffi::OsString,
    fs,
    io::ErrorKind,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const LOCAL_PROFILE_NAME: &str = "local";

const ADMIN_USER: &str = "pgsandbox_admin";
const CONFIG_FILE_NAME: &str = "local-postgres.json";
const DATA_DIR: &str = "postgres/data";
const LOCK_FILE_NAME: &str = "local-postgres.lock";
const LOG_FILE: &str = "postgres/postgres.log";
const PASSWORD_FILE: &str = "postgres/initdb-password";
const DEFAULT_LOCAL_PORT: u16 = 65432;
#[cfg(test)]
const REQUIRED_LOCAL_BINARIES: &[&str] = &["initdb", "pg_ctl", "postgres"];
const HOMEBREW_OPT_ROOTS: &[&str] = &["/opt/homebrew/opt", "/usr/local/opt"];
const COMMON_POSTGRES_MAJOR_VERSIONS: &[&str] = &["18", "17", "16", "15", "14", "13"];
const COMMON_LOCAL_POSTGRES_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/opt/postgresql/bin",
    "/usr/local/opt/postgresql/bin",
    "/Applications/Postgres.app/Contents/Versions/latest/bin",
];
const POSTGRES_CONF_BEGIN: &str = "# BEGIN PGSandbox local runtime";
const POSTGRES_CONF_END: &str = "# END PGSandbox local runtime";

#[derive(Clone, Debug)]
pub struct LocalPostgresCluster {
    root: PathBuf,
    postgres_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalClusterConfig {
    pub profile_name: String,
    #[serde(default)]
    pub postgres_version: Option<String>,
    #[serde(default)]
    pub postgres_bin_dir: Option<PathBuf>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPostgresInstallation {
    pub postgres_version: String,
    pub postgres_bin_dir: Option<PathBuf>,
    pub source: String,
}

struct LocalClusterLock {
    _file: fs::File,
}

impl LocalPostgresCluster {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_env_for_version(None)
    }

    pub fn from_env_for_version(postgres_version: Option<&str>) -> anyhow::Result<Self> {
        let root = match std::env::var_os("PGSANDBOX_HOME") {
            Some(path) => PathBuf::from(path),
            None => dirs::home_dir()
                .context("could not resolve home directory for ~/.pgsandbox")?
                .join(".pgsandbox"),
        };
        Self::new_for_version(root, postgres_version)
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            postgres_version: None,
        }
    }

    pub fn new_for_version(
        root: impl Into<PathBuf>,
        postgres_version: Option<&str>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            root: root.into(),
            postgres_version: postgres_version
                .map(normalize_postgres_version)
                .transpose()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(self.config_file_name())
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
            self.ensure_data_dir_matches_requested_version()?;
            return self.read_config();
        }

        let binaries = resolve_postgres_binaries(self.postgres_version.as_deref())?;
        fs::create_dir_all(self.socket_dir()).with_context(|| {
            format!(
                "failed to create local Postgres socket directory {}",
                self.socket_dir().display()
            )
        })?;

        let password = local_admin_password();
        let mut config =
            self.config_for_port_with_binaries(select_free_port()?, &password, &binaries);
        write_password_file(&self.password_file(), &password)?;

        let init_result = Command::new(&binaries.initdb)
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
        config.socket_dir = self.socket_dir();
        let binaries =
            resolve_postgres_binaries_for_config(&config, self.postgres_version.as_deref())?;
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

        if let Err(error) = command_output(
            "pg_ctl",
            Command::new(&binaries.pg_ctl)
                .arg("-D")
                .arg(&config.data_dir)
                .arg("-l")
                .arg(&config.log_file)
                .arg("-w")
                .arg("start")
                .output(),
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

        let config = self.read_config()?;
        let binaries =
            resolve_postgres_binaries_for_config(&config, self.postgres_version.as_deref())?;
        command_output(
            "pg_ctl",
            Command::new(&binaries.pg_ctl)
                .arg("-D")
                .arg(&config.data_dir)
                .arg("-m")
                .arg("fast")
                .arg("-w")
                .arg("stop")
                .output(),
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

    #[cfg(test)]
    fn config_for_port(&self, port: u16, password: &str) -> LocalClusterConfig {
        LocalClusterConfig {
            profile_name: profile_name_for_version(self.postgres_version.as_deref()),
            postgres_version: self.postgres_version.clone(),
            postgres_bin_dir: None,
            admin_url: admin_url_for(port, password),
            host: "127.0.0.1".to_string(),
            port,
            data_dir: self.data_dir(),
            socket_dir: self.socket_dir(),
            log_file: self.log_file(),
        }
    }

    fn config_for_port_with_binaries(
        &self,
        port: u16,
        password: &str,
        binaries: &LocalPostgresBinaries,
    ) -> LocalClusterConfig {
        LocalClusterConfig {
            profile_name: profile_name_for_version(self.postgres_version.as_deref()),
            postgres_version: Some(binaries.version.clone()),
            postgres_bin_dir: binaries.bin_dir.clone(),
            admin_url: admin_url_for(port, password),
            host: "127.0.0.1".to_string(),
            port,
            data_dir: self.data_dir(),
            socket_dir: self.socket_dir(),
            log_file: self.log_file(),
        }
    }

    fn data_dir(&self) -> PathBuf {
        match &self.postgres_version {
            Some(version) => self
                .root
                .join("postgres")
                .join("versions")
                .join(version)
                .join("data"),
            None => self.root.join(DATA_DIR),
        }
    }

    fn log_file(&self) -> PathBuf {
        match &self.postgres_version {
            Some(version) => self
                .root
                .join("postgres")
                .join("versions")
                .join(version)
                .join("postgres.log"),
            None => self.root.join(LOG_FILE),
        }
    }

    fn lock_path(&self) -> PathBuf {
        match &self.postgres_version {
            Some(version) => self.root.join(format!("local-postgres-{version}.lock")),
            None => self.root.join(LOCK_FILE_NAME),
        }
    }

    fn password_file(&self) -> PathBuf {
        match &self.postgres_version {
            Some(version) => self
                .root
                .join("postgres")
                .join("versions")
                .join(version)
                .join("initdb-password"),
            None => self.root.join(PASSWORD_FILE),
        }
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
        let config: LocalClusterConfig = serde_json::from_str(&raw).with_context(|| {
            format!(
                "local Postgres config file is not valid JSON at {}",
                path.display()
            )
        })?;
        if let Some(requested) = &self.postgres_version {
            if config.postgres_version.as_deref() != Some(requested) {
                anyhow::bail!(
                    "local Postgres config {} is for version {:?}, not requested version {requested}",
                    path.display(),
                    config.postgres_version
                );
            }
        }
        Ok(config)
    }

    fn socket_dir(&self) -> PathBuf {
        short_socket_root().join(socket_dir_id(&self.root, self.postgres_version.as_deref()))
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

        let config = self.read_config()?;
        let binaries =
            resolve_postgres_binaries_for_config(&config, self.postgres_version.as_deref())?;
        match Command::new(&binaries.pg_ctl)
            .arg("-D")
            .arg(self.data_dir())
            .arg("status")
            .output()
        {
            Ok(output) => Ok(output.status.success()),
            Err(error) if error.kind() == ErrorKind::NotFound => missing_local_postgres_binaries(),
            Err(error) => Err(error).context("failed to run pg_ctl status"),
        }
    }

    fn config_file_name(&self) -> String {
        match &self.postgres_version {
            Some(version) => format!("local-postgres-{version}.json"),
            None => CONFIG_FILE_NAME.to_string(),
        }
    }

    fn ensure_data_dir_matches_requested_version(&self) -> anyhow::Result<()> {
        let Some(requested) = &self.postgres_version else {
            return Ok(());
        };
        let path = self.data_dir().join("PG_VERSION");
        let actual = fs::read_to_string(&path)
            .with_context(|| format!("failed to read local Postgres version {}", path.display()))?;
        let actual = normalize_postgres_version(actual.trim())?;
        if &actual != requested {
            anyhow::bail!(
                "local Postgres data directory {} was initialized with version {actual}, not requested version {requested}",
                self.data_dir().display()
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalPostgresBinaries {
    initdb: PathBuf,
    pg_ctl: PathBuf,
    postgres: PathBuf,
    bin_dir: Option<PathBuf>,
    version: String,
}

fn profile_name_for_version(postgres_version: Option<&str>) -> String {
    match postgres_version {
        Some(version) => format!("local-pg{version}"),
        None => LOCAL_PROFILE_NAME.to_string(),
    }
}

fn normalize_postgres_version(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    let major = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    if major.is_empty() {
        anyhow::bail!("postgresVersion must start with a numeric major version");
    }
    Ok(major)
}

fn versioned_bin_dir_env_key(postgres_version: &str) -> anyhow::Result<String> {
    let version = normalize_postgres_version(postgres_version)?;
    Ok(format!("PGSANDBOX_POSTGRES_{version}_BIN_DIR"))
}

fn resolve_postgres_binaries(
    requested_version: Option<&str>,
) -> anyhow::Result<LocalPostgresBinaries> {
    let requested_version = requested_version
        .map(normalize_postgres_version)
        .transpose()?;
    let mut failures = Vec::new();

    for candidate in binary_candidates(requested_version.as_deref()) {
        match candidate.resolve() {
            Ok(binaries)
                if requested_version
                    .as_deref()
                    .is_none_or(|requested| binaries.version == requested) =>
            {
                return Ok(binaries);
            }
            Ok(binaries) => failures.push(format!(
                "{} resolved Postgres {}, not requested {}",
                candidate.label(),
                binaries.version,
                requested_version.as_deref().unwrap_or_default()
            )),
            Err(error) => failures.push(format!("{}: {error:#}", candidate.label())),
        }
    }

    match requested_version {
        Some(version) => anyhow::bail!(
            "could not find local Postgres {version} binaries. Set {} or put matching initdb, pg_ctl, and postgres on PATH. Tried: {}",
            versioned_bin_dir_env_key(&version)?,
            failures.join("; ")
        ),
        None => anyhow::bail!(
            "could not find local Postgres binaries. Install PostgreSQL locally or set PGSANDBOX_POSTGRES_BIN_DIR. Tried: {}",
            failures.join("; ")
        ),
    }
}

fn resolve_postgres_binaries_for_config(
    config: &LocalClusterConfig,
    requested_version: Option<&str>,
) -> anyhow::Result<LocalPostgresBinaries> {
    let expected_version = requested_version
        .map(normalize_postgres_version)
        .transpose()?
        .or_else(|| config.postgres_version.clone());

    let binaries = if let Some(bin_dir) = &config.postgres_bin_dir {
        let saved_candidate = LocalPostgresBinaryCandidate::BinDir {
            label: format!("saved postgresBinDir {}", bin_dir.display()),
            path: bin_dir.clone(),
        };
        match saved_candidate.resolve() {
            Ok(binaries) => binaries,
            Err(saved_error) => resolve_postgres_binaries(expected_version.as_deref())
                .with_context(|| {
                    format!(
                        "{} could not be used: {saved_error:#}",
                        saved_candidate.label()
                    )
                })?,
        }
    } else {
        resolve_postgres_binaries(expected_version.as_deref())?
    };

    if let Some(expected_version) = expected_version {
        anyhow::ensure!(
            binaries.version == expected_version,
            "local Postgres config for profile {} expects version {}, but {} reports version {}",
            config.profile_name,
            expected_version,
            binaries.pg_ctl.display(),
            binaries.version
        );
    }

    Ok(binaries)
}

#[derive(Debug, Clone)]
enum LocalPostgresBinaryCandidate {
    BinDir { label: String, path: PathBuf },
    PathCommands,
}

impl LocalPostgresBinaryCandidate {
    fn label(&self) -> String {
        match self {
            Self::BinDir { label, .. } => label.clone(),
            Self::PathCommands => "PATH".to_string(),
        }
    }

    fn resolve(&self) -> anyhow::Result<LocalPostgresBinaries> {
        match self {
            Self::BinDir { path, .. } => postgres_binaries_from_dir(path),
            Self::PathCommands => postgres_binaries_from_commands(
                PathBuf::from("initdb"),
                PathBuf::from("pg_ctl"),
                PathBuf::from("postgres"),
                None,
            ),
        }
    }
}

fn binary_candidates(requested_version: Option<&str>) -> Vec<LocalPostgresBinaryCandidate> {
    let mut candidates = Vec::new();
    let mut seen_dirs = BTreeSet::new();

    if let Some(version) = requested_version {
        if let Ok(key) = versioned_bin_dir_env_key(version) {
            if let Some(path) = env::var_os(&key) {
                push_candidate_dir(&mut candidates, &mut seen_dirs, key, PathBuf::from(path));
            }
        }
    }

    for (key, path) in versioned_bin_dir_env_candidates() {
        push_candidate_dir(&mut candidates, &mut seen_dirs, key, path);
    }

    if let Some(path) = env::var_os("PGSANDBOX_POSTGRES_BIN_DIR") {
        push_candidate_dir(
            &mut candidates,
            &mut seen_dirs,
            "PGSANDBOX_POSTGRES_BIN_DIR".to_string(),
            PathBuf::from(path),
        );
    }

    if let Some(version) = requested_version {
        for path in common_postgres_bin_dirs(version) {
            push_candidate_dir(
                &mut candidates,
                &mut seen_dirs,
                format!("common bin dir {}", path.display()),
                path,
            );
        }
    }

    for path in local_postgres_bin_dirs(env::var_os("PATH")) {
        push_candidate_dir(
            &mut candidates,
            &mut seen_dirs,
            format!("local bin dir {}", path.display()),
            path,
        );
    }
    candidates.push(LocalPostgresBinaryCandidate::PathCommands);
    candidates
}

fn common_postgres_bin_dirs(postgres_version: &str) -> Vec<PathBuf> {
    vec![
        format!("/opt/homebrew/opt/postgresql@{postgres_version}/bin").into(),
        format!("/usr/local/opt/postgresql@{postgres_version}/bin").into(),
        format!("/Applications/Postgres.app/Contents/Versions/{postgres_version}/bin").into(),
        format!("/usr/lib/postgresql/{postgres_version}/bin").into(),
        format!("/opt/local/lib/postgresql{postgres_version}/bin").into(),
    ]
}

fn push_candidate_dir(
    candidates: &mut Vec<LocalPostgresBinaryCandidate>,
    seen_dirs: &mut BTreeSet<PathBuf>,
    label: String,
    path: PathBuf,
) {
    if seen_dirs.insert(path.clone()) {
        candidates.push(LocalPostgresBinaryCandidate::BinDir { label, path });
    }
}

fn postgres_binaries_from_dir(path: &Path) -> anyhow::Result<LocalPostgresBinaries> {
    postgres_binaries_from_commands(
        path.join("initdb"),
        path.join("pg_ctl"),
        path.join("postgres"),
        Some(path.to_path_buf()),
    )
}

fn postgres_binaries_from_commands(
    initdb: PathBuf,
    pg_ctl: PathBuf,
    postgres: PathBuf,
    bin_dir: Option<PathBuf>,
) -> anyhow::Result<LocalPostgresBinaries> {
    let _ = postgres_binary_output("initdb", &initdb)?;
    let _ = postgres_binary_output("pg_ctl", &pg_ctl)?;
    let postgres_output = postgres_binary_output("postgres", &postgres)?;
    let version = postgres_major_version_from_output(&postgres, &postgres_output)?;
    Ok(LocalPostgresBinaries {
        initdb,
        pg_ctl,
        postgres,
        bin_dir,
        version,
    })
}

fn postgres_binary_output(binary: &'static str, path: &Path) -> anyhow::Result<Output> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run `{}` --version", path.display()))?;
    if !output.status.success() {
        failed_command(
            &path.display().to_string(),
            output.status,
            command_failure_output(&output),
        )
        .with_context(|| format!("Postgres binary `{binary}` failed version check"))?;
    }
    Ok(output)
}

fn postgres_major_version_from_output(postgres: &Path, output: &Output) -> anyhow::Result<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_postgres_major_version(&stdout)
        .or_else(|| parse_postgres_major_version(&stderr))
        .with_context(|| {
            format!(
                "could not parse Postgres version from {}",
                postgres.display()
            )
        })
}

fn parse_postgres_major_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find_map(|part| normalize_postgres_version(part).ok())
}

pub fn discover_local_postgres_installations() -> Vec<LocalPostgresInstallation> {
    let mut installations = Vec::new();
    for candidate in discovery_binary_candidates() {
        let label = candidate.label();
        if let Ok(binaries) = candidate.resolve() {
            if installations
                .iter()
                .any(|installation: &LocalPostgresInstallation| {
                    installation.postgres_version == binaries.version
                        && installation.postgres_bin_dir == binaries.bin_dir
                })
            {
                continue;
            }
            installations.push(LocalPostgresInstallation {
                postgres_version: binaries.version,
                postgres_bin_dir: binaries.bin_dir,
                source: label,
            });
        }
    }
    installations.sort_by(|left, right| {
        left.postgres_version
            .cmp(&right.postgres_version)
            .then_with(|| left.source.cmp(&right.source))
    });
    installations
}

fn discovery_binary_candidates() -> Vec<LocalPostgresBinaryCandidate> {
    let mut candidates = Vec::new();
    let mut seen_dirs = BTreeSet::new();
    for (key, path) in versioned_bin_dir_env_candidates() {
        push_candidate_dir(&mut candidates, &mut seen_dirs, key, path);
    }
    if let Some(path) = env::var_os("PGSANDBOX_POSTGRES_BIN_DIR") {
        push_candidate_dir(
            &mut candidates,
            &mut seen_dirs,
            "PGSANDBOX_POSTGRES_BIN_DIR".to_string(),
            PathBuf::from(path),
        );
    }
    for path in local_postgres_bin_dirs(env::var_os("PATH")) {
        push_candidate_dir(
            &mut candidates,
            &mut seen_dirs,
            format!("local bin dir {}", path.display()),
            path,
        );
    }
    candidates.push(LocalPostgresBinaryCandidate::PathCommands);
    for path in discovered_common_postgres_bin_dirs() {
        push_candidate_dir(
            &mut candidates,
            &mut seen_dirs,
            format!("common bin dir {}", path.display()),
            path,
        );
    }
    candidates
}

fn versioned_bin_dir_env_candidates() -> Vec<(String, PathBuf)> {
    let mut candidates = env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            let version = key
                .strip_prefix("PGSANDBOX_POSTGRES_")?
                .strip_suffix("_BIN_DIR")?;
            if normalize_postgres_version(version).ok()?.as_str() != version {
                return None;
            }
            Some((key, PathBuf::from(value)))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates
}

fn discovered_common_postgres_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = BTreeSet::new();

    for version in COMMON_POSTGRES_MAJOR_VERSIONS {
        for path in common_postgres_bin_dirs(version) {
            push_unique_dir(&mut dirs, &mut seen, path);
        }
    }

    push_child_bin_dirs(
        &mut dirs,
        &mut seen,
        Path::new("/usr/lib/postgresql"),
        |_| true,
    );
    push_child_bin_dirs(
        &mut dirs,
        &mut seen,
        Path::new("/Applications/Postgres.app/Contents/Versions"),
        |name| name != "latest",
    );
    push_child_bin_dirs(&mut dirs, &mut seen, Path::new("/opt/local/lib"), |name| {
        name.starts_with("postgresql")
    });

    dirs
}

fn push_child_bin_dirs<F>(
    dirs: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
    root: &Path,
    accept: F,
) where
    F: Fn(&str) -> bool,
{
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if accept(&name) {
            push_unique_dir(dirs, seen, entry.path().join("bin"));
        }
    }
}

pub(crate) fn select_free_port_with_probe<F>(mut available: F) -> Option<u16>
where
    F: FnMut(u16) -> bool,
{
    (DEFAULT_LOCAL_PORT..=u16::MAX).find(|port| available(*port))
}

#[cfg(test)]
fn required_local_binaries() -> &'static [&'static str] {
    REQUIRED_LOCAL_BINARIES
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

fn short_socket_root() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp/pgsandbox-sockets")
    }
    #[cfg(not(unix))]
    {
        env::temp_dir().join("pgsandbox-sockets")
    }
}

fn socket_dir_id(root: &Path, postgres_version: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(postgres_version.unwrap_or("default").as_bytes());
    let digest = hasher.finalize();
    let suffix = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    match postgres_version {
        Some(version) => format!("pg{version}-{suffix}"),
        None => format!("default-{suffix}"),
    }
}

#[cfg(test)]
fn probe_local_postgres_bin_dir(dirs: Vec<PathBuf>) -> anyhow::Result<PathBuf> {
    let mut first_failure = None;

    'candidate: for dir in dirs {
        if !required_local_binaries()
            .iter()
            .all(|binary| dir.join(binary).is_file())
        {
            continue;
        }
        for binary in required_local_binaries() {
            let path = dir.join(binary);
            match Command::new(&path).arg("--version").output() {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    first_failure.get_or_insert_with(|| {
                        format!(
                            "Postgres binary `{}` is installed at {} but failed to run: {}",
                            binary,
                            path.display(),
                            summarize_stderr(command_failure_output(&output))
                        )
                    });
                    continue 'candidate;
                }
                Err(error) if error.kind() == ErrorKind::NotFound => continue 'candidate,
                Err(error) => {
                    first_failure.get_or_insert_with(|| {
                        format!("failed to run `{} --version`: {error}", path.display())
                    });
                    continue 'candidate;
                }
            }
        }
        return Ok(dir);
    }

    if let Some(first_failure) = first_failure {
        anyhow::bail!(
            "Postgres server binaries `initdb`, `pg_ctl`, and `postgres` were found in local install locations, but no complete set ran successfully. First failure: {first_failure}"
        );
    }

    missing_local_postgres_binaries()
}

fn local_postgres_bin_dirs(path: Option<OsString>) -> Vec<PathBuf> {
    let homebrew_roots = HOMEBREW_OPT_ROOTS
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    local_postgres_bin_dirs_with_roots(path, &homebrew_roots)
}

fn local_postgres_bin_dirs_with_roots(
    path: Option<OsString>,
    homebrew_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut dirs = Vec::new();
    if let Some(path) = path {
        for dir in std::env::split_paths(&path) {
            push_unique_dir(&mut dirs, &mut seen, dir);
        }
    }
    for dir in homebrew_postgres_bin_dirs(homebrew_roots) {
        push_unique_dir(&mut dirs, &mut seen, dir);
    }
    for dir in COMMON_LOCAL_POSTGRES_BIN_DIRS {
        push_unique_dir(&mut dirs, &mut seen, PathBuf::from(dir));
    }
    dirs
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, dir: PathBuf) {
    if seen.insert(dir.clone()) {
        dirs.push(dir);
    }
}

fn homebrew_postgres_bin_dirs(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut versioned = Vec::new();
    let mut unversioned = Vec::new();

    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "postgresql" {
                unversioned.push(entry.path().join("bin"));
                continue;
            }
            if let Some(version) = name
                .strip_prefix("postgresql@")
                .and_then(|version| version.parse::<u32>().ok())
            {
                versioned.push((version, entry.path().join("bin")));
            }
        }
    }

    versioned.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    unversioned.sort();

    versioned
        .into_iter()
        .map(|(_, path)| path)
        .chain(unversioned)
        .collect()
}

fn missing_local_postgres_binaries<T>() -> anyhow::Result<T> {
    anyhow::bail!(
        "Postgres server binaries `initdb`, `pg_ctl`, and `postgres` were not found together on PATH or in common local install locations. Install PostgreSQL locally so PGSandbox can manage ~/.pgsandbox/postgres without using Docker."
    )
}

fn command_output(
    binary: &'static str,
    result: std::io::Result<std::process::Output>,
) -> anyhow::Result<()> {
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => failed_command(binary, output.status, command_failure_output(&output)),
        Err(error) if error.kind() == ErrorKind::NotFound => missing_local_postgres_binaries(),
        Err(error) => Err(error).with_context(|| format!("failed to start `{binary}`")),
    }
}

fn command_failure_output(output: &Output) -> &[u8] {
    if output.stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        &output.stderr
    } else {
        &output.stdout
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
    fn local_postgres_binary_search_keeps_path_order_and_common_installs() {
        let path = std::env::join_paths([
            PathBuf::from("/tmp/pg-one"),
            PathBuf::from("/tmp/pg-two"),
            PathBuf::from("/tmp/pg-one"),
        ])
        .unwrap();
        let dirs = local_postgres_bin_dirs(Some(path));

        assert_eq!(dirs[0], PathBuf::from("/tmp/pg-one"));
        assert_eq!(dirs[1], PathBuf::from("/tmp/pg-two"));
        assert!(dirs.contains(&PathBuf::from(
            "/Applications/Postgres.app/Contents/Versions/latest/bin"
        )));
    }

    #[test]
    fn homebrew_postgres_binary_search_discovers_future_versions() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("postgresql@17/bin")).unwrap();
        fs::create_dir_all(directory.path().join("postgresql@19/bin")).unwrap();
        fs::create_dir_all(directory.path().join("postgresql/bin")).unwrap();
        fs::create_dir_all(directory.path().join("not-postgres@20/bin")).unwrap();

        let dirs = local_postgres_bin_dirs_with_roots(None, &[directory.path().to_path_buf()]);

        assert_eq!(dirs[0], directory.path().join("postgresql@19/bin"));
        assert_eq!(dirs[1], directory.path().join("postgresql@17/bin"));
        assert_eq!(dirs[2], directory.path().join("postgresql/bin"));
    }

    #[test]
    fn common_discovery_probes_postgres_versions_through_13() {
        let dirs = discovered_common_postgres_bin_dirs();

        for version in ["18", "17", "16", "15", "14", "13"] {
            assert!(
                dirs.contains(&PathBuf::from(format!(
                    "/opt/homebrew/opt/postgresql@{version}/bin"
                ))),
                "expected Homebrew probe for Postgres {version}"
            );
            assert!(
                dirs.contains(&PathBuf::from(format!(
                    "/Applications/Postgres.app/Contents/Versions/{version}/bin"
                ))),
                "expected Postgres.app probe for Postgres {version}"
            );
            assert!(
                dirs.contains(&PathBuf::from(format!("/usr/lib/postgresql/{version}/bin"))),
                "expected Debian/Ubuntu probe for Postgres {version}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn postgres_binary_probe_skips_broken_candidate_directories() {
        let directory = tempfile::tempdir().unwrap();
        let broken = directory.path().join("broken");
        let working = directory.path().join("working");
        fs::create_dir_all(&broken).unwrap();
        fs::create_dir_all(&working).unwrap();

        for binary in required_local_binaries() {
            write_executable(&broken.join(binary), "#!/bin/sh\necho broken\nexit 1\n");
            write_executable(&working.join(binary), "#!/bin/sh\necho version\nexit 0\n");
        }

        let resolved = probe_local_postgres_bin_dir(vec![broken, working.clone()]).unwrap();

        assert_eq!(resolved, working);
    }

    #[cfg(unix)]
    #[test]
    fn saved_bin_dir_falls_back_to_matching_discovered_binaries() {
        let directory = tempfile::tempdir().unwrap();
        let stale = directory.path().join("stale");
        let working = directory.path().join("working");
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir_all(&working).unwrap();

        for binary in required_local_binaries() {
            write_executable(
                &working.join(binary),
                "#!/bin/sh\necho 'postgres (PostgreSQL) 99.1'\nexit 0\n",
            );
        }

        let key = "PGSANDBOX_POSTGRES_99_BIN_DIR";
        let previous = env::var_os(key);
        env::set_var(key, &working);
        let config = LocalClusterConfig {
            profile_name: "local-pg99".to_string(),
            postgres_version: Some("99".to_string()),
            postgres_bin_dir: Some(stale),
            admin_url: admin_url_for(65432, "secret"),
            host: "127.0.0.1".to_string(),
            port: 65432,
            data_dir: directory.path().join("data"),
            socket_dir: directory.path().join("run"),
            log_file: directory.path().join("postgres.log"),
        };

        let binaries = resolve_postgres_binaries_for_config(&config, Some("99")).unwrap();

        match previous {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
        assert_eq!(binaries.bin_dir.as_deref(), Some(working.as_path()));
        assert_eq!(binaries.version, "99");
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
        assert!(saved.socket_dir.starts_with(short_socket_root()));
        assert_eq!(saved.admin_url, config.admin_url);
    }

    #[test]
    fn versioned_cluster_uses_separate_profile_config_and_runtime_paths() {
        let directory = tempfile::tempdir().unwrap();
        let cluster = LocalPostgresCluster::new_for_version(directory.path(), Some("16")).unwrap();
        let config = cluster.config_for_port(65432, "secret");

        assert_eq!(
            cluster.config_path(),
            directory.path().join("local-postgres-16.json")
        );
        assert_eq!(
            cluster.lock_path(),
            directory.path().join("local-postgres-16.lock")
        );
        assert_eq!(config.profile_name, "local-pg16");
        assert_eq!(config.postgres_version.as_deref(), Some("16"));
        assert_eq!(
            config.data_dir,
            directory.path().join("postgres/versions/16/data")
        );
        assert!(config.socket_dir.starts_with(short_socket_root()));
        assert_eq!(
            config.log_file,
            directory.path().join("postgres/versions/16/postgres.log")
        );
    }

    #[test]
    fn socket_dir_uses_short_tmp_path_for_deep_state_roots() {
        let deep_root = PathBuf::from(format!(
            "/var/folders/mx/{}/T/pgsandbox-fail-log-{}",
            "nested".repeat(12),
            "suffix".repeat(8)
        ));
        let cluster = LocalPostgresCluster::new(deep_root);
        let socket_dir = cluster.socket_dir();

        assert!(socket_dir.starts_with(short_socket_root()));
        assert!(
            socket_dir.join(".s.PGSQL.65435").to_string_lossy().len() < 103,
            "socket path should fit macOS Postgres unix socket limit: {}",
            socket_dir.display()
        );
    }

    #[test]
    fn local_bin_dir_env_key_is_version_specific() {
        assert_eq!(
            versioned_bin_dir_env_key("16").unwrap(),
            "PGSANDBOX_POSTGRES_16_BIN_DIR"
        );
        assert_eq!(
            versioned_bin_dir_env_key("16.4").unwrap(),
            "PGSANDBOX_POSTGRES_16_BIN_DIR"
        );
    }

    #[test]
    fn discovery_includes_versioned_bin_dir_env_vars() {
        let directory = tempfile::tempdir().unwrap();
        let key = "PGSANDBOX_POSTGRES_99_BIN_DIR";
        let previous = env::var_os(key);
        env::set_var(key, directory.path());

        let candidates = versioned_bin_dir_env_candidates();

        match previous {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
        assert!(candidates
            .iter()
            .any(|(candidate_key, path)| candidate_key == key && path == directory.path()));
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

    #[cfg(unix)]
    fn write_executable(path: &Path, content: &str) {
        use std::{io::Write, os::unix::fs::PermissionsExt};

        let mut file = fs::File::create(path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}
