// Refs:
// - https://doc.rust-lang.org/nightly/cargo/commands/cargo-clean.html
// - https://github.com/rust-lang/cargo/blob/0.62.0/src/cargo/ops/cargo_clean.rs

use std::path::Path;

use anyhow::Result;
use cargo_metadata::PackageId;
use walkdir::WalkDir;

use crate::{
    cargo::{self, Workspace},
    cli::Args,
    context::Context,
    fs,
    regex_vec::{RegexVec, RegexVecBuilder},
    term,
};

pub(crate) fn run(options: &mut Args) -> Result<()> {
    let ws = Workspace::new(&options.manifest, None, false, false)?;
    ws.config.merge_to_args(&mut None, &mut options.build.verbose, &mut options.build.color);
    term::set_coloring(&mut options.build.color);

    if !options.workspace {
        for dir in &[&ws.target_dir, &ws.output_dir] {
            rm_rf(dir, options.build.verbose != 0)?;
        }
        return Ok(());
    }

    clean_ws(&ws, &ws.metadata.workspace_members, options.build.verbose)?;

    Ok(())
}

// TODO: remove need for this.
// If --no-clean, --no-run, or --no-report is used: do not remove artifacts
// Otherwise, remove the followings to avoid false positives/false negatives:
// - build artifacts of crates to be measured for coverage
// - profdata
// - profraw
// - doctest bins
// - old reports
pub(crate) fn clean_partial(cx: &Context) -> Result<()> {
    if cx.build.no_clean {
        return Ok(());
    }

    clean_ws_inner(&cx.ws, &cx.workspace_members.included, cx.build.verbose > 1)?;

    let package_args: Vec<_> = cx
        .workspace_members
        .included
        .iter()
        .flat_map(|id| ["--package", &cx.ws.metadata[id].name])
        .collect();
    let mut cmd = cx.cargo();
    cmd.arg("clean").args(&package_args);
    cargo::clean_args(cx, &mut cmd);
    if let Err(e) = if cx.build.verbose > 1 { cmd.run() } else { cmd.run_with_output() } {
        warn!("{e:#}");
    }

    Ok(())
}

fn clean_ws(ws: &Workspace, pkg_ids: &[PackageId], verbose: u8) -> Result<()> {
    clean_ws_inner(ws, pkg_ids, verbose != 0)?;

    let package_args: Vec<_> =
        pkg_ids.iter().flat_map(|id| ["--package", &ws.metadata[id].name]).collect();
    let mut args_set = vec![vec![]];
    if ws.target_dir.join("release").exists() {
        args_set.push(vec!["--release"]);
    }
    let target_list = ws.rustc_print("target-list")?;
    for target in target_list.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if ws.target_dir.join(target).exists() {
            args_set.push(vec!["--target", target]);
        }
    }
    for args in args_set {
        let mut cmd = ws.cargo(verbose);
        cmd.args(["clean", "--target-dir", ws.target_dir.as_str()]).args(&package_args);
        cmd.args(args);
        if verbose > 0 {
            cmd.arg(format!("-{}", "v".repeat(verbose as usize)));
        }
        cmd.dir(&ws.metadata.workspace_root);
        if let Err(e) = if verbose > 0 { cmd.run() } else { cmd.run_with_output() } {
            warn!("{e:#}");
        }
    }
    Ok(())
}

fn clean_ws_inner(ws: &Workspace, pkg_ids: &[PackageId], verbose: bool) -> Result<()> {
    for format in &["html", "text"] {
        rm_rf(ws.output_dir.join(format), verbose)?;
    }

    for path in glob::glob(ws.target_dir.join("*.profraw").as_str())?.filter_map(Result::ok) {
        rm_rf(path, verbose)?;
    }

    rm_rf(&ws.doctests_dir, verbose)?;
    rm_rf(&ws.profdata_file, verbose)?;

    clean_trybuild_artifacts(ws, pkg_ids, verbose)?;
    Ok(())
}

fn pkg_hash_re(ws: &Workspace, pkg_ids: &[PackageId]) -> RegexVec {
    let mut re = RegexVecBuilder::new("^(lib)?(", ")(-[0-9a-f]{7,})?$");
    for id in pkg_ids {
        re.or(&ws.metadata[id].name.replace('-', "(-|_)"));
    }
    re.build().unwrap()
}

fn clean_trybuild_artifacts(ws: &Workspace, pkg_ids: &[PackageId], verbose: bool) -> Result<()> {
    let trybuild_dir = &ws.metadata.target_directory.join("tests");
    let trybuild_target = &trybuild_dir.join("target");
    let re = pkg_hash_re(ws, pkg_ids);

    for e in WalkDir::new(trybuild_target).into_iter().filter_map(Result::ok) {
        let path = e.path();
        if let Some(file_stem) = fs::file_stem_recursive(path).unwrap().to_str() {
            if re.is_match(file_stem) {
                rm_rf(path, verbose)?;
            }
        }
    }
    Ok(())
}

fn rm_rf(path: impl AsRef<Path>, verbose: bool) -> Result<()> {
    let path = path.as_ref();
    let m = fs::symlink_metadata(path);
    if m.as_ref().map(fs::Metadata::is_dir).unwrap_or(false) {
        if verbose {
            status!("Removing", "{}", path.display());
        }
        fs::remove_dir_all(path)?;
    } else if m.is_ok() {
        if verbose {
            status!("Removing", "{}", path.display());
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
