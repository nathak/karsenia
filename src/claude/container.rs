//! Running a provider chat's `claude` inside a docker or podman container with only its project folder mounted.
//!
//! EXPERIMENTAL: written against the documented behaviour of `docker run` and `podman run`, and tested here only with a stand-in
//! runtime that records what it was asked; no real container runtime was available where this was written.
//!
//! Only provider chats may use it. An account chat would need the account's credentials inside the container, where a command the
//! chat runs could read them, which would defeat the purpose. The container is a second wall around the commands and file reads a
//! chat is allowed to make (the chat sees the project folder and nothing of the rest of the home folder, the data folder or the
//! account's credentials). It does not protect the project folder itself (mounted read-write: what a command writes there, such as a
//! `.claude/settings.json` or a hook, is read later by an account chat in the same folder), and the network is open.
use std::path::{Path, PathBuf};

/// Where the config folder of a chat is inside the container.
pub const CONFIG_IN: &str = "/tenrec/config";
const NAME_PREFIX: &str = "tenrec-";
/// The label of the data folder a container carries, and the one a build of the first name put on its containers: a sweep looks
/// for both, so one that outlived the rename (a SIGKILL) is still found.
const DATA_LABEL: &str = "tenrec.data";
const OLD_DATA_LABEL: &str = "swb.data";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub program: PathBuf,
    pub image: String,
}

/// What `ClaudeProcess` needs to start the container and to remove it again.
#[derive(Debug, Clone)]
pub struct Spec {
    pub runtime: Runtime,
    /// `tenrec-<chat id>-<random>`: the name `rm -f` is given when the client that started it cannot stop it (a SIGKILL).
    pub name: String,
    /// The same for every container of one data folder, so a later start can find the ones a crash left.
    pub data_hash: String,
    pub config_dir: PathBuf,
}

/// An absolute path to `docker` or `podman` (by file name; the person types it, never a chat).
pub fn validate_runtime(text: &str) -> Result<PathBuf, &'static str> {
    let path = Path::new(text.trim());
    if !path.is_absolute() || !path.is_file() {
        return Err("the runtime must be the absolute path of a file that exists");
    }
    match path.file_name().and_then(|n| n.to_str()) {
        Some("docker" | "podman") => Ok(path.to_path_buf()),
        _ => Err("the runtime file must be named docker or podman"),
    }
}

/// An image name or id as `docker run` takes it; no flag can hide in it.
pub fn validate_image(text: &str) -> Result<String, &'static str> {
    let image = text.trim();
    if image.is_empty() || image.len() > 200 || image.starts_with(['-', '.', '/']) || !image.chars().all(|c| c.is_ascii_alphanumeric() || "._/:@-".contains(c)) {
        return Err("the image name holds a character that is not allowed");
    }
    Ok(image.to_owned())
}

/// A folder that goes into a `-v src:dst` argument: it must not be able to carry a second option into it.
fn mountable(path: &Path) -> Result<String, &'static str> {
    let text = path.to_str().ok_or("a folder name is not plain text")?;
    if !path.is_absolute() || text.chars().any(|c| c.is_control() || c == ':' || c == ',' || c == '\\') {
        return Err("this folder's path holds a character a mount cannot take (: , or a backslash)");
    }
    Ok(text.to_owned())
}

/// `tenrec-<chat id>-<instance>-<random>`: `instance` is one random tag per run of this app, so a sweep at start can tell the
/// containers it must leave alone (this run's) from those an earlier run left.
pub fn container_name(sid: &str, instance: &str, random: &str) -> String {
    format!("{NAME_PREFIX}{sid}-{instance}-{random}")
}

/// A project folder a container may be given: not the data folder, not a folder that holds it, not the home folder or a folder that
/// holds it (the account's credentials and `~/.claude.json` are there), and not a place in the home folder that keeps secrets.
/// Paths are compared as given, so pass canonical ones.
pub fn project_allowed(project: &Path, data_dir: &Path, home: Option<&Path>, runtime_dir: Option<&Path>) -> Result<(), &'static str> {
    let data = data_dir.canonicalize().unwrap_or_else(|_| data_dir.to_path_buf());
    if project.starts_with(&data) || data.starts_with(project) {
        return Err("a container cannot be given the app's data folder, or a folder that holds it");
    }
    // Without a home folder the rules below cannot be applied, so nothing is given away on a guess.
    let Some(home) = home.and_then(|h| h.canonicalize().ok()) else {
        return Err("the home folder is not known, so a container chat cannot be checked");
    };
    if home.starts_with(project) {
        return Err("a container cannot be given the home folder or a folder that holds it: the account's credentials are there");
    }
    if [".claude", ".ssh", ".aws", ".gnupg", ".config", ".kube", ".docker", ".local"].iter().any(|dir| project.starts_with(home.join(dir))) {
        return Err("a container cannot be given a folder where the home folder keeps credentials and settings");
    }
    // Folders where this user's programs keep sockets (tmux, ssh-agent, gpg-agent, the container runtime's own): a container that runs
    // as the same user could drive them. A project that holds one of them is refused, and so is one inside the run folders.
    let mut sockets: Vec<PathBuf> = ["/tmp", "/var/tmp", "/dev/shm", "/run", "/var/run"].iter().map(PathBuf::from).collect();
    sockets.extend(runtime_dir.and_then(|d| d.canonicalize().ok()));
    if sockets.iter().any(|dir| dir.starts_with(project)) {
        return Err("a container cannot be given /tmp, /run or a folder that holds them: programs keep sockets there that it could drive");
    }
    let runtime = runtime_dir.and_then(|d| d.canonicalize().ok());
    if project.starts_with("/run") || project.starts_with("/var/run") || runtime.is_some_and(|r| project.starts_with(r)) {
        return Err("a container cannot be given a folder in /run or the user's runtime folder: programs keep sockets there that it could drive");
    }
    Ok(())
}

/// One number for one data folder (FNV-1a): the label by which the containers of this app are found again.
pub fn data_hash(dir: &Path) -> String {
    // The same folder reached by another spelling (a link, a trailing slash) is the same folder.
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let h = dir.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// The arguments after the runtime's program name. `env_names` are passed by NAME only (`-e NAME`): the value comes from the
/// environment of the client process, so a key is never in an argument (arguments of a process are readable by every user).
pub fn run_args(spec: &Spec, cwd: &Path, env_names: &[String], claude_args: &[String], uid: u32, gid: u32) -> Result<Vec<String>, &'static str> {
    let project = mountable(cwd)?;
    let config = mountable(&spec.config_dir)?;
    let podman = spec.runtime.program.file_name().and_then(|n| n.to_str()) == Some("podman");
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "-i",
        "--init",
        // A mistyped image name must fail, not fetch some other image from a registry.
        "--pull=never",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=512m",
        "--pids-limit=512",
        "--memory=4g",
    ]
    .iter()
    .map(|a| (*a).to_owned())
    .collect();
    args.extend(["--name".into(), spec.name.clone(), "--label".into(), "tenrec.app=1".into(), "--label".into(), format!("{DATA_LABEL}={}", spec.data_hash)]);
    if podman {
        args.push("--userns=keep-id".into());
    }
    args.extend(["--user".into(), format!("{uid}:{gid}")]);
    args.extend(["-v".into(), format!("{project}:{project}:rw"), "-v".into(), format!("{config}:{CONFIG_IN}:rw"), "-w".into(), project]);
    args.extend(["-e".into(), "HOME=/tmp".into(), "-e".into(), format!("CLAUDE_CONFIG_DIR={CONFIG_IN}")]);
    for name in env_names {
        // Names only: a value after `=` would put a secret into the arguments.
        if name.contains('=') || name.is_empty() {
            return Err("an environment name is not valid");
        }
        args.extend(["-e".into(), name.clone()]);
    }
    args.push(spec.runtime.image.clone());
    args.push("claude".into());
    args.extend(claude_args.iter().cloned());
    Ok(args)
}

/// `rm -f <name>`: removes the container whether it runs or not. The command is started at once; `wait` is how long to wait for it
/// (zero: do not wait, which is right when this process may be about to end: the command goes on without it).
pub fn remove(program: &Path, name: &str, wait: std::time::Duration) {
    let Ok(mut child) = std::process::Command::new(program).args(["rm", "-f", name]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() else { return };
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Still going: leave it to finish by itself, and reap it in the background so it does not stay a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Removes the containers an earlier run of this app left (found by the label of this data folder). The ones of this run
/// (`instance`) are not touched, so a sweep that is late cannot remove a chat that has just started.
pub fn sweep(program: &Path, data_hash: &str, instance: &str) {
    let own = format!("-{instance}-");
    for label in [DATA_LABEL, OLD_DATA_LABEL] {
        let Ok(listing) = std::process::Command::new(program).args(["ps", "-a", "--filter", &format!("label={label}={data_hash}"), "--format", "{{.ID}} {{.Names}}"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else { continue };
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            let mut parts = line.split_whitespace();
            let (Some(id), name) = (parts.next(), parts.next().unwrap_or_default()) else { continue };
            if id.chars().all(|c| c.is_ascii_alphanumeric()) && !name.contains(&own) {
                remove(program, id, std::time::Duration::from_secs(5));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str) -> Spec {
        Spec { runtime: Runtime { program: PathBuf::from(program), image: "my-claude:latest".into() }, name: "tenrec-ab12-cd34".into(), data_hash: "00ff".into(), config_dir: PathBuf::from("/data/chatcfg/ab12") }
    }

    fn has(args: &[String], pair: &[&str]) -> bool {
        args.windows(pair.len()).any(|w| w.iter().zip(pair).all(|(a, b)| a == b))
    }

    #[test]
    fn the_container_is_closed_in_and_holds_only_the_project_and_its_own_settings() {
        let claude = vec!["-p".to_owned(), "--settings".to_owned(), "{}".to_owned()];
        let args = run_args(&spec("/usr/bin/docker"), Path::new("/home/me/proj"), &["ANTHROPIC_BASE_URL".into(), "ANTHROPIC_AUTH_TOKEN".into()], &claude, 1000, 1000).unwrap();
        assert_eq!(&args[..3], ["run", "--rm", "-i"], "an interactive pipe (the permission prompts), removed when it ends");
        for flag in ["--init", "--pull=never", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--read-only", "--pids-limit=512", "--memory=4g"] {
            assert!(args.iter().any(|a| a == flag), "{flag} missing: {args:?}");
        }
        assert!(has(&args, &["--name", "tenrec-ab12-cd34"]) && has(&args, &["--label", "tenrec.app=1"]) && has(&args, &["--label", "tenrec.data=00ff"]));
        assert!(has(&args, &["--user", "1000:1000"]));
        assert!(!args.iter().any(|a| a.starts_with("--userns")), "docker has no keep-id");
        // Exactly two mounts: the project at its own path, and the chat's own settings folder.
        let mounts: Vec<&String> = args.iter().enumerate().filter(|(i, a)| *i > 0 && args[*i - 1] == "-v" && !a.is_empty()).map(|(_, a)| a).collect();
        assert_eq!(mounts, ["/home/me/proj:/home/me/proj:rw", "/data/chatcfg/ab12:/tenrec/config:rw"]);
        assert!(has(&args, &["-w", "/home/me/proj"]));
        for banned in ["--network=host", "--network", "--privileged", "--volumes-from", "--device"] {
            assert!(!args.iter().any(|a| a.starts_with(banned)), "{banned}");
        }
        assert!(!args.iter().any(|a| a == "/data" || a.starts_with("/data:") || a == "/home/me" || a.contains(".claude")), "the data folder and the home folder are not mounted: {args:?}");
        // Variables go by name only, and the image comes right before claude and its arguments.
        assert!(has(&args, &["-e", "ANTHROPIC_AUTH_TOKEN"]) && has(&args, &["-e", "CLAUDE_CONFIG_DIR=/tenrec/config"]) && has(&args, &["-e", "HOME=/tmp"]));
        let image = args.iter().position(|a| a == "my-claude:latest").unwrap();
        assert_eq!(&args[image + 1..], ["claude", "-p", "--settings", "{}"]);
        assert!(args[..image].iter().filter(|a| a.contains('=') && a.starts_with("ANTHROPIC")).count() == 0, "no value of a credential variable in the arguments");
    }

    #[test]
    fn podman_keeps_the_users_id_and_odd_paths_names_and_images_are_refused() {
        let args = run_args(&spec("/usr/bin/podman"), Path::new("/p"), &[], &[], 1000, 1000).unwrap();
        assert!(args.iter().any(|a| a == "--userns=keep-id"));
        for bad in ["/a:b", "/a,b", "/a\\b", "/a\nb", "relative"] {
            assert!(run_args(&spec("/usr/bin/docker"), Path::new(bad), &[], &[], 1, 1).is_err(), "{bad:?}");
        }
        let mut odd = spec("/usr/bin/docker");
        odd.config_dir = PathBuf::from("/data:evil/x");
        assert!(run_args(&odd, Path::new("/p"), &[], &[], 1, 1).is_err(), "the settings folder's path is checked as well");
        assert!(run_args(&spec("/usr/bin/docker"), Path::new("/p"), &["A=b".into()], &[], 1, 1).is_err(), "a name with a value would put a secret in the arguments");
        for bad in ["", "-x", "--privileged", "a b", "a;b", "$(x)", "/abs", ".hidden", &"a".repeat(201)] {
            assert!(validate_image(bad).is_err(), "{bad:?}");
        }
        for good in ["claude", "my-claude:latest", "ghcr.io/me/claude@sha256:abc123", "registry.local:5000/team/claude:1.2"] {
            assert_eq!(validate_image(good).as_deref(), Ok(good));
        }
    }

    #[test]
    fn a_project_may_not_expose_the_data_folder_or_the_home_folders_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home").join("me");
        let data = home.join(".local").join("share").join("app");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(home.join(".claude").join("sub")).unwrap();
        std::fs::create_dir_all(home.join("work").join("proj")).unwrap();
        let allowed = |p: &Path| project_allowed(p, &data, Some(&home), None);
        assert!(allowed(&home.join("work").join("proj")).is_ok());
        assert!(allowed(&root.join("elsewhere")).is_ok());
        for bad in [&data, &data.join("chatcfg"), data.parent().unwrap(), &home, home.parent().unwrap(), &root, &home.join(".claude"), &home.join(".claude").join("sub"), &home.join(".local")] {
            assert!(allowed(bad).is_err(), "{} must be refused", bad.display());
        }
        // With the data folder elsewhere, the home rules stand on their own.
        let far = root.join("far-data");
        std::fs::create_dir_all(&far).unwrap();
        let alone = |p: &Path| project_allowed(p, &far, Some(&home), None);
        assert!(alone(&home.join("work")).is_ok());
        for bad in [&home, home.parent().unwrap(), &root, &home.join(".claude"), &home.join(".ssh")] {
            assert!(alone(bad).is_err(), "{} must be refused", bad.display());
        }
        // Folders of sockets, and a home folder that is not known.
        for bad in ["/tmp", "/var/tmp", "/dev/shm", "/run", "/run/user", "/var", "/run/user/1000/podman"] {
            assert!(alone(Path::new(bad)).is_err(), "{bad} must be refused");
        }
        let runtime = root.join("xdg-run");
        std::fs::create_dir_all(runtime.join("podman")).unwrap();
        assert!(project_allowed(&runtime, &far, Some(&home), Some(&runtime)).is_err());
        assert!(project_allowed(&runtime.join("podman"), &far, Some(&home), Some(&runtime)).is_err());
        assert!(project_allowed(&home.join("work"), &far, Some(&home), Some(&runtime)).is_ok());
        assert!(project_allowed(Path::new("/tmp/some-project"), &far, Some(&home), None).is_ok(), "a project that merely lives in /tmp is fine");
        assert!(project_allowed(&home.join("work"), &far, None, None).is_err(), "no home folder known: no container chat");
    }

    #[test]
    fn a_late_sweep_leaves_this_runs_containers_alone_and_never_passes_an_odd_id_on() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let ps = dir.path().join("ps.txt");
        let docker = dir.path().join("docker");
        std::fs::write(&docker, format!("#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1\" = ps ]; then cat '{}'; fi\n", log.display(), ps.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&ps, "old1 tenrec-a-111111-aaa\nmine1 tenrec-b-222222-bbb\nold2 tenrec-c-333333-ccc\n$(touch x) tenrec-d-111111-ddd\n").unwrap();
        sweep(&docker, "00ff", "222222");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("rm -f old1") && calls.contains("rm -f old2"));
        assert!(!calls.contains("mine1"), "a container of this run was removed: {calls}");
        assert!(!calls.contains("touch"), "{calls}");
    }

    #[test]
    fn a_container_labelled_by_the_first_name_of_the_app_is_swept_too() {
        let dir = tempfile::tempdir().unwrap();
        let (log, ps) = (dir.path().join("calls.log"), dir.path().join("ps-old.txt"));
        let docker = dir.path().join("docker");
        // `ps` answers only for the old label: the new one finds nothing.
        std::fs::write(&docker, format!("#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1\" = ps ]; then case \"$*\" in *label=swb.data=00ff*) cat '{}';; esac; fi\n", log.display(), ps.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&ps, "left1 swb-a-111111-aaa\n").unwrap();
        sweep(&docker, "00ff", "222222");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("label=tenrec.data=00ff") && calls.contains("label=swb.data=00ff"), "{calls}");
        assert!(calls.contains("rm -f left1"), "{calls}");
    }

    #[test]
    fn only_a_file_named_docker_or_podman_is_a_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let docker = dir.path().join("docker");
        let other = dir.path().join("evil");
        std::fs::write(&docker, "#!/bin/sh\n").unwrap();
        std::fs::write(&other, "#!/bin/sh\n").unwrap();
        assert_eq!(validate_runtime(docker.to_str().unwrap()).unwrap(), docker);
        assert!(validate_runtime(other.to_str().unwrap()).is_err());
        assert!(validate_runtime("docker").is_err(), "a bare name is looked up in PATH, which is not what the person meant");
        assert!(validate_runtime(dir.path().join("podman").to_str().unwrap()).is_err(), "a file that does not exist");
        assert!(validate_runtime(dir.path().to_str().unwrap()).is_err());
        assert_eq!(data_hash(Path::new("/a")), data_hash(Path::new("/a")));
        assert_ne!(data_hash(Path::new("/a")), data_hash(Path::new("/b")));
    }
}
