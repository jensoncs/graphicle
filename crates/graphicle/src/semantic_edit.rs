//! Local-only semantic previews. Application recomputes the preview from current
//! inputs and atomically replaces one file; no remote capability can edit a checkout.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Inspect Go conditions and scalar value flow in one function.
    StatementFlow {
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        file: String,
        #[arg(long)]
        byte_offset: usize,
        #[command(flatten)]
        profile: Profile,
    },
    /// Preview a function-local Go variable/parameter rename, without writes.
    RenamePreview {
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        file: String,
        #[arg(long)]
        byte_offset: usize,
        #[arg(long)]
        new_name: String,
        #[command(flatten)]
        profile: Profile,
    },
    /// Revalidate a saved preview and atomically replace its one source file.
    RenameApply {
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        preview: PathBuf,
    },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    #[arg(long, default_value = "")]
    pub goos: String,
    #[serde(default)]
    #[arg(long, default_value = "")]
    pub goarch: String,
    #[serde(default)]
    #[arg(long = "tag")]
    pub tags: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    format: String,
    root: String,
    file: String,
    offset: usize,
    new_name: String,
    profile: Profile,
    inputs: BTreeMap<String, String>,
    analyzer: String,
    analysis: Value,
    preview_id: String,
}
fn relative(file: &str) -> Result<&Path> {
    let path = Path::new(file);
    if file.is_empty()
        || file.len() > 1024
        || file.contains('\\')
        || !file.ends_with(".go")
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_none_or(|s| s == "secrets" || s.starts_with(".env"))
        })
    {
        return Err(Error::bad("Expected a safe relative Go source path"));
    }
    Ok(path)
}
fn no_links(root: &Path, path: &Path) -> Result<()> {
    let mut part = root.to_path_buf();
    for c in path.components() {
        part.push(c);
        if fs::symlink_metadata(&part)?.file_type().is_symlink() {
            return Err(Error::bad("Semantic edits refuse symbolic links"));
        }
    }
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = vec![];
    options
        .open(path)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(Error::bad("Semantic source exceeds 2 MiB"));
    }
    Ok(bytes)
}
fn capture(
    root: &Path,
    file: &str,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let relative = relative(file)?;
    no_links(root, relative)?;
    let directory = relative.parent().unwrap_or(Path::new(""));
    let mut files = BTreeMap::new();
    let mut inputs = BTreeMap::new();
    let mut bytes = 0usize;
    for entry in fs::read_dir(root.join(directory))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "go") {
            continue;
        }
        let name = path
            .strip_prefix(root)
            .map_err(|_| Error::bad("Source outside root"))?
            .to_string_lossy()
            .replace('\\', "/");
        relative_path_check(root, &name)?;
        let body = read(&path)?;
        bytes += body.len();
        if bytes > 2 * 1024 * 1024 || files.len() >= 128 {
            return Err(Error::bad("Semantic package exceeds source limits"));
        }
        inputs.insert(name.clone(), graphicle_memory::text::sha256(&body));
        files.insert(
            name,
            String::from_utf8(body).map_err(|_| Error::bad("Go sources must be UTF-8"))?,
        );
    }
    // Include absent configuration too: creating a go.work or go.mod must revoke
    // the preview just as editing/removing an existing one does.
    let mut dir = Some(directory);
    while let Some(parent) = dir {
        for name in [
            "go.mod",
            "go.sum",
            "go.work",
            "go.work.sum",
            "vendor/modules.txt",
        ] {
            let rel = parent.join(name);
            let key = rel.to_string_lossy().replace('\\', "/");
            let path = root.join(&rel);
            if path.exists() || path.is_symlink() {
                no_links(root, &rel)?;
                let content = read(&path)?;
                bytes += content.len();
                if bytes > 4 * 1024 * 1024 {
                    return Err(Error::bad("Source and configuration exceed 4 MiB"));
                }
                inputs.insert(key, graphicle_memory::text::sha256(&content));
            } else {
                inputs.insert(key, "absent".into());
            }
        }
        dir = parent.parent();
    }
    Ok((files, inputs))
}
fn relative_path_check(root: &Path, file: &str) -> Result<()> {
    no_links(root, relative(file)?)
}
fn root(path: &Path) -> Result<PathBuf> {
    if path.is_symlink() {
        return Err(Error::bad("Root cannot be a symlink"));
    }
    let root = path.canonicalize()?;
    if !root.is_dir() {
        return Err(Error::bad("Root must be a directory"));
    }
    Ok(root)
}
fn analyzer() -> Result<String> {
    Ok(crate::digest(
        &graphicle::extractors::go_types::review_identity()?,
    ))
}
pub fn preview(
    path: &Path,
    file: &str,
    offset: usize,
    new_name: &str,
    profile: Profile,
) -> Result<Value> {
    let root = root(path)?;
    let (files, inputs) = capture(&root, file)?;
    if new_name.len() > 256 || profile.tags.len() > 32 || profile.tags.iter().any(|s| s.len() > 100)
    {
        return Err(Error::bad("Rename request exceeds limits"));
    }
    let analysis = graphicle::extractors::go_types::semantic(
        &json!({"mode":"rename","files":files,"file":file,"offset":offset,"new_name":new_name,"goos":profile.goos,"goarch":profile.goarch,"tags":profile.tags}),
    )?;
    if analysis["validated"] != true {
        return Err(Error::conflict(format!(
            "Rename refused: {}",
            analysis["error"].as_str().unwrap_or("incomplete analysis")
        )));
    }
    let mut plan = serde_json::to_value(Plan {
        format: "graphicle.rename.v1".into(),
        root: root.to_string_lossy().into_owned(),
        file: file.into(),
        offset,
        new_name: new_name.into(),
        profile,
        inputs,
        analyzer: analyzer()?,
        analysis,
        preview_id: String::new(),
    })?;
    plan["preview_id"] = json!(crate::digest(&plan));
    Ok(plan)
}
pub fn apply(path: &Path, expected: &Value) -> Result<Value> {
    let plan: Plan = serde_json::from_value(expected.clone())?;
    let root = root(path)?;
    if plan.format != "graphicle.rename.v1" || plan.root != root.to_string_lossy() {
        return Err(Error::conflict(
            "Preview belongs to a different root or format",
        ));
    }
    no_links(&root, relative(&plan.file)?)?;
    let target = root.join(&plan.file);
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let locked = options.open(&target)?;
    locked
        .try_lock()
        .map_err(|_| Error::conflict("Source is locked by another edit"))?;
    let (_, current_inputs) = capture(&root, &plan.file)?;
    if current_inputs != plan.inputs || analyzer()? != plan.analyzer {
        return Err(Error::conflict(
            "Source or configuration changed; request a fresh preview",
        ));
    }
    let fresh = preview(&root, &plan.file, plan.offset, &plan.new_name, plan.profile)?;
    if fresh != *expected {
        return Err(Error::conflict(
            "Source, configuration, analyzer or preview changed; request a fresh preview",
        ));
    }
    let original = read(&target)?;
    let original =
        String::from_utf8(original).map_err(|_| Error::bad("Invalid source encoding"))?;
    let mut replacement = String::new();
    let mut cursor = 0;
    for edit in fresh["analysis"]["edits"]
        .as_array()
        .ok_or_else(|| Error::bad("Missing validated edits"))?
    {
        let start = edit["byte_start"]
            .as_u64()
            .ok_or_else(|| Error::bad("Invalid edit"))? as usize;
        let end = edit["byte_end"]
            .as_u64()
            .ok_or_else(|| Error::bad("Invalid edit"))? as usize;
        if edit["file"] != plan.file
            || start < cursor
            || original.get(start..end) != edit["before"].as_str()
        {
            return Err(Error::conflict("Validated edit no longer matches"));
        }
        replacement.push_str(
            original
                .get(cursor..start)
                .ok_or_else(|| Error::bad("Invalid edit span"))?,
        );
        replacement.push_str(
            edit["after"]
                .as_str()
                .ok_or_else(|| Error::bad("Invalid replacement"))?,
        );
        cursor = end;
    }
    replacement.push_str(&original[cursor..]);
    let mut temporary = tempfile::Builder::new()
        .prefix(".graphicle-rename-")
        .tempfile_in(target.parent().unwrap())?;
    temporary
        .as_file()
        .set_permissions(locked.metadata()?.permissions())?;
    temporary.write_all(replacement.as_bytes())?;
    temporary.as_file().sync_all()?;
    let (_, inputs) = capture(&root, &plan.file)?;
    if inputs != plan.inputs || analyzer()? != plan.analyzer {
        return Err(Error::conflict(
            "Inputs changed during apply; request a fresh preview",
        ));
    }
    temporary
        .persist(&target)
        .map_err(|e| Error::bad(format!("Atomic rename failed: {}", e.error)))?;
    Ok(
        json!({"applied":true,"file":plan.file,"preview_id":plan.preview_id,"sha256":graphicle_memory::text::sha256(replacement.as_bytes()),"reindex_required":true}),
    )
}
pub fn run(command: Command) -> Result<Value> {
    match command {
        Command::RenamePreview {
            path,
            file,
            byte_offset,
            new_name,
            profile,
        } => preview(&path, &file, byte_offset, &new_name, profile),
        Command::RenameApply { path, preview } => {
            apply(&path, &crate::review::read_request(&preview)?)
        }
        Command::StatementFlow {
            path,
            file,
            byte_offset,
            profile,
        } => {
            let root = root(&path)?;
            let (files, inputs) = capture(&root, &file)?;
            let mut result = graphicle::extractors::go_types::semantic(
                &json!({"mode":"flow","files":files,"file":file,"offset":byte_offset,"goos":profile.goos,"goarch":profile.goarch,"tags":profile.tags}),
            )?;
            result["inputs"] = json!(inputs);
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CODE: &str = "package app\nfunc F(value int) int { other := value + 1; return other }\n";
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.go"), CODE).unwrap();
        dir
    }
    #[test]
    fn rename_preview_is_exact_and_apply_revalidates_without_touching_unrelated_names() {
        let dir = fixture();
        let plan = preview(
            dir.path(),
            "main.go",
            CODE.find("value int").unwrap(),
            "input",
            Profile::default(),
        )
        .unwrap();
        assert_eq!(plan["analysis"]["edits"].as_array().unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join("main.go")).unwrap(),
            CODE
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                dir.path().join("main.go"),
                fs::Permissions::from_mode(0o640),
            )
            .unwrap();
        }
        assert_eq!(apply(dir.path(), &plan).unwrap()["applied"], true);
        assert_eq!(
            fs::read_to_string(dir.path().join("main.go")).unwrap(),
            CODE.replace("value", "input")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join("main.go"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }
        assert!(apply(dir.path(), &plan).is_err());
    }
    #[test]
    fn stale_source_new_config_and_forged_edits_never_apply() {
        for change in [
            "source",
            "config",
            "deleted_config",
            "forged",
            "profile",
            "new_file",
            "deleted_file",
        ] {
            let dir = fixture();
            fs::write(dir.path().join("go.mod"), "module example/app\ngo 1.23\n").unwrap();
            fs::write(dir.path().join("extra.go"), "package app\n").unwrap();
            let mut plan = preview(
                dir.path(),
                "main.go",
                CODE.find("value int").unwrap(),
                "input",
                Profile::default(),
            )
            .unwrap();
            match change {
                "source" => {
                    fs::write(dir.path().join("main.go"), CODE.replace("+ 1", "+ 2")).unwrap()
                }
                "config" => fs::write(dir.path().join("go.work"), "go 1.23\nuse .\n").unwrap(),
                "deleted_config" => fs::remove_file(dir.path().join("go.mod")).unwrap(),
                "deleted_file" => fs::remove_file(dir.path().join("extra.go")).unwrap(),
                "new_file" => fs::write(dir.path().join("new.go"), "package app\n").unwrap(),
                "profile" => plan["profile"]["tags"] = json!(["changed"]),
                _ => plan["analysis"]["edits"][0]["after"] = json!("forged"),
            }
            let before = fs::read(dir.path().join("main.go")).unwrap();
            assert!(apply(dir.path(), &plan).is_err(), "{change}");
            assert_eq!(fs::read(dir.path().join("main.go")).unwrap(), before);
        }
    }
    #[test]
    fn collisions_captures_and_unsupported_targets_are_refused() {
        let dir = fixture();
        assert!(
            preview(
                dir.path(),
                "main.go",
                CODE.find("value int").unwrap(),
                "other",
                Profile::default()
            )
            .is_err()
        );
        let code = "package app\nfunc F(value []int) int { return len(value) }\n";
        fs::write(dir.path().join("main.go"), code).unwrap();
        assert!(
            preview(
                dir.path(),
                "main.go",
                code.find("value").unwrap(),
                "len",
                Profile::default()
            )
            .is_err()
        );
        assert!(
            preview(
                dir.path(),
                "main.go",
                code.find("F(").unwrap(),
                "Other",
                Profile::default()
            )
            .is_err()
        );
        assert!(preview(dir.path(), "../main.go", 0, "x", Profile::default()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let dir = fixture();
        std::os::unix::fs::symlink(dir.path().join("main.go"), dir.path().join("alias.go"))
            .unwrap();
        assert!(
            preview(
                dir.path(),
                "alias.go",
                CODE.find("value").unwrap(),
                "input",
                Profile::default()
            )
            .is_err()
        );
    }
}
