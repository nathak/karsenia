//! Running a provider chat's `claude` inside a docker or podman container with only its project folder mounted.
//!
//! EXPERIMENTAL: written against the documented behaviour of `docker run` and `podman run`, and tested here only with a stand-in
//! runtime that records what it was asked; no real container runtime was available where this was written.
//!
//! Only provider chats may use it. An account chat would need the account's credentials inside the container, where a command the
//! chat runs could read them, which would defeat the purpose. The container is a second wall around the commands and file reads a
//! chat is allowed to make (the chat sees the project folder and nothing of the rest of the home folder, the data folder or the
//! account's credentials). It does not protect the project folder itself (mounted read-write), and the network is open.
use std::path::{Path, PathBuf};

/// Where the config folder of a chat is inside the container.
pub const CONFIG_IN: &str = "/swb/config";
const NAME_PREFIX: &str = "swb-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub program: PathBuf,
    pub image: String,
}

/// What `ClaudeProcess` needs to start the container and to remove it again.
#[derive(Debug, Clone)]
pub struct Spec {
    pub runtime: Runtime,
    /// `swb-<chat id>-<random>`: the name `rm -f` is given when the client that started it cannot stop it (a SIGKILL).
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

pub fn container_name(sid: &str, random: &str) -> String {
    format!("{NAME_PREFIX}{sid}-{random}")
}

/// One number for one data folder (FNV-1a): the label by which the containers of this app are found again.
pub fn data_hash(dir: &Path) -> String {
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
    args.extend(["--name".into(), spec.name.clone(), "--label".into(), "swb.app=1".into(), "--label".into(), format!("swb.data={}", spec.data_hash)]);
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

/// `rm -f <name>`: removes the container whether it runs or not. Waits at most `wait` for it.
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

/// Removes the containers a crash of an earlier run of this app left (found by the label of this data folder).
pub fn sweep(program: &Path, data_hash: &str) {
    let Ok(listing) = std::process::Command::new(program).args(["ps", "-aq", "--filter", &format!("label=swb.data={data_hash}")]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else { return };
    for id in String::from_utf8_lossy(&listing.stdout).split_whitespace().filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric())) {
        remove(program, id, std::time::Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str) -> Spec {
        Spec { runtime: Runtime { program: PathBuf::from(program), image: "my-claude:latest".into() }, name: "swb-ab12-cd34".into(), data_hash: "00ff".into(), config_dir: PathBuf::from("/data/chatcfg/ab12") }
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
        assert!(has(&args, &["--name", "swb-ab12-cd34"]) && has(&args, &["--label", "swb.app=1"]) && has(&args, &["--label", "swb.data=00ff"]));
        assert!(has(&args, &["--user", "1000:1000"]));
        assert!(!args.iter().any(|a| a.starts_with("--userns")), "docker has no keep-id");
        // Exactly two mounts: the project at its own path, and the chat's own settings folder.
        let mounts: Vec<&String> = args.iter().enumerate().filter(|(i, a)| *i > 0 && args[*i - 1] == "-v" && !a.is_empty()).map(|(_, a)| a).collect();
        assert_eq!(mounts, ["/home/me/proj:/home/me/proj:rw", "/data/chatcfg/ab12:/swb/config:rw"]);
        assert!(has(&args, &["-w", "/home/me/proj"]));
        for banned in ["--network=host", "--network", "--privileged", "--volumes-from", "--device"] {
            assert!(!args.iter().any(|a| a.starts_with(banned)), "{banned}");
        }
        assert!(!args.iter().any(|a| a == "/data" || a.starts_with("/data:") || a == "/home/me" || a.contains(".claude")), "the data folder and the home folder are not mounted: {args:?}");
        // Variables go by name only, and the image comes right before claude and its arguments.
        assert!(has(&args, &["-e", "ANTHROPIC_AUTH_TOKEN"]) && has(&args, &["-e", "CLAUDE_CONFIG_DIR=/swb/config"]) && has(&args, &["-e", "HOME=/tmp"]));
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
