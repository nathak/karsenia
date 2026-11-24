fn main() {
    // Only the desktop app has a UI to compile; the library and the web server do not need Slint.
    #[cfg(feature = "desktop")]
    {
        let config = slint_build::CompilerConfiguration::new().embed_resources(slint_build::EmbedResourcesKind::EmbedFiles).with_style("fluent".into());
        slint_build::compile_with_config("slint/app.slint", config).expect("the Slint UI did not compile");
    }
    windows_icon();
    git_revision();
}

/// The commit this build was made from (`TENREC_GIT`, with a `+` when tracked files differ from it), shown in Settings -> About and in the
/// log, so that a person can tell which build is running. Without git (a source archive) it is `unknown`.
fn git_revision() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    let git = |args: &[&str]| std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success());
    let hash = git(&["rev-parse", "--short", "HEAD"]).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|h| !h.is_empty()).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["diff", "--quiet", "HEAD"]).is_none() && hash != "unknown";
    println!("cargo:rustc-env=TENREC_GIT={hash}{}", if dirty { "+" } else { "" });
}

/// The program file's own icon (what Explorer and the taskbar show for the .exe): `assets/icon.ico` written as a Windows resource file
/// (`.res`) and handed to the linker of the desktop app. A failure only costs the icon: it is reported as a warning and the build goes on.
fn windows_icon() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || std::env::var_os("CARGO_FEATURE_DESKTOP").is_none() {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("tenrec-icon.res");
    match std::fs::read("assets/icon.ico").map_err(|e| e.to_string()).and_then(|ico| icon_resource(&ico)).and_then(|res| std::fs::write(&out, res).map_err(|e| e.to_string())) {
        Ok(()) => println!("cargo:rustc-link-arg-bin=karsenia-desktop={}", out.display()),
        Err(why) => println!("cargo:warning=the program icon was not embedded: {why}"),
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| "icon.ico is cut short".to_owned())
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "icon.ico is cut short".to_owned())
}

/// One entry of a `.res` file: a resource of an ordinal type and name (type 3 is one picture of an icon, 14 the group that lists them).
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    // The header: sizes, the type and the name as ordinals (0xFFFF and the number), then version, flags, language and the rest.
    out.extend(32u32.to_le_bytes());
    for ordinal in [kind, name] {
        out.extend(0xFFFFu16.to_le_bytes());
        out.extend(ordinal.to_le_bytes());
    }
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(0x0409u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// The pictures of an `.ico` file as the resources of a program: one `RT_ICON` for each, and the `RT_GROUP_ICON` (named 1) that lists them.
fn icon_resource(ico: &[u8]) -> Result<Vec<u8>, String> {
    if u16_at(ico, 0)? != 0 || u16_at(ico, 2)? != 1 {
        return Err("icon.ico is not an icon file".to_owned());
    }
    let count = u16_at(ico, 4)? as usize;
    // A resource file starts with an empty resource, so that it is told from the older format.
    let mut out = Vec::new();
    out.extend(0u32.to_le_bytes());
    out.extend(32u32.to_le_bytes());
    out.extend([0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    out.extend([0u8; 16]);
    let mut group = Vec::new();
    group.extend([0, 0, 1, 0]);
    group.extend((count as u16).to_le_bytes());
    for i in 0..count {
        let entry = 6 + i * 16;
        let (size, offset) = (u32_at(ico, entry + 8)? as usize, u32_at(ico, entry + 12)? as usize);
        let picture = ico.get(offset..offset + size).ok_or_else(|| "icon.ico is cut short".to_owned())?;
        resource(&mut out, 3, i as u16 + 1, 0x1010, picture);
        // The group's entry is the file's entry without the offset, with the number of the picture in its place.
        let mut head = [0u8; 12];
        head.copy_from_slice(&ico[entry..entry + 12]);
        // One plane and 32 bits when the file leaves them out (a PNG picture): the system picks the size by these.
        if u16_at(ico, entry + 4)? == 0 {
            head[4..6].copy_from_slice(&1u16.to_le_bytes());
        }
        if u16_at(ico, entry + 6)? == 0 {
            head[6..8].copy_from_slice(&32u16.to_le_bytes());
        }
        group.extend(head);
        group.extend((i as u16 + 1).to_le_bytes());
    }
    resource(&mut out, 14, 1, 0x1030, &group);
    Ok(out)
}
