//
// compress.rs
// GitHub Replicant (Rust)
//
// Compresses folders inside a target directory into individual .zip files.
// Supports recursive mode to compress folders at a specified depth level.
//
// Thales Matheus Mendonça Santos - December 2025

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::FileOptions;
use zip::ZipWriter;

/// Tool to compress folders inside a target directory into individual .zip files.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// The target folder containing directories to compress
    #[arg(short, long)]
    input: PathBuf,

    /// Recursion depth level (0 = immediate children, 1 = grandchildren, etc.)
    #[arg(short, long)]
    recursive: Option<usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let input_path = cli.input.canonicalize().context(format!(
        "Failed to resolve input path: {}",
        cli.input.display()
    ))?;

    if !input_path.is_dir() {
        anyhow::bail!("Input path is not a directory: {}", input_path.display());
    }

    let depth = cli.recursive.unwrap_or(0);

    println!(
        "Compressing folders at depth {} inside: {}",
        depth,
        input_path.display()
    );

    let folders_to_compress = collect_folders_at_depth(&input_path, depth)?;

    if folders_to_compress.is_empty() {
        println!("No folders found to compress at the specified depth.");
        return Ok(());
    }

    println!("Found {} folders to compress.", folders_to_compress.len());

    let pb = ProgressBar::new(folders_to_compress.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    for folder in &folders_to_compress {
        let folder_name = folder
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        pb.set_message(format!("Compressing: {}", folder_name));

        let zip_path = folder.with_extension("zip");

        if let Err(e) = compress_folder(folder, &zip_path) {
            pb.println(format!("Error compressing {}: {}", folder.display(), e));
        }

        pb.inc(1);
    }

    pb.finish_with_message("Done!");

    println!(
        "Successfully compressed {} folders.",
        folders_to_compress.len()
    );

    Ok(())
}

/// Collects all folders at a specific depth level relative to the root.
/// depth=0 means immediate children of root.
/// depth=1 means children of the immediate children (grandchildren).
fn collect_folders_at_depth(root: &Path, depth: usize) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();

    if depth == 0 {
        // Collect immediate child directories
        for entry in fs::read_dir(root).context("Failed to read directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                result.push(path);
            }
        }
    } else {
        // First, get immediate children
        for entry in fs::read_dir(root).context("Failed to read directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Recursively collect at depth - 1
                let sub_folders = collect_folders_at_depth(&path, depth - 1)?;
                result.extend(sub_folders);
            }
        }
    }

    Ok(result)
}

/// Compresses a folder into a .zip file.
fn compress_folder(folder: &Path, zip_path: &Path) -> Result<()> {
    let file = File::create(zip_path).context(format!(
        "Failed to create zip file: {}",
        zip_path.display()
    ))?;

    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let folder_name = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string());

    for entry in WalkDir::new(folder).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let relative_path = path.strip_prefix(folder).unwrap_or(path);

        // Build the path inside the zip with the folder name as root
        let zip_internal_path = PathBuf::from(&folder_name).join(relative_path);
        let zip_internal_str = zip_internal_path.to_string_lossy();

        if path.is_file() {
            zip.start_file(zip_internal_str.as_ref(), options)?;

            let mut f = File::open(path)?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        } else if path.is_dir() && path != folder {
            // Add directory entry (trailing slash)
            let dir_path = format!("{}/", zip_internal_str);
            zip.add_directory(&dir_path, options)?;
        }
    }

    zip.finish()?;
    Ok(())
}
