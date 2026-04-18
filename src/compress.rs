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
use std::io::BufReader;
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
    let file = File::create(zip_path)
        .context(format!("Failed to create zip file: {}", zip_path.display()))?;

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

            let f = File::open(path)
                .with_context(|| format!("Failed to open file: {}", path.display()))?;
            let mut reader = BufReader::new(f);
            std::io::copy(&mut reader, &mut zip)
                .with_context(|| format!("Failed to write file to zip: {}", path.display()))?;
        } else if path.is_dir() && path != folder {
            // Add directory entry (trailing slash)
            let dir_path = format!("{}/", zip_internal_str);
            zip.add_directory(&dir_path, options)?;
        }
    }

    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tempfile::tempdir;
    use zip::ZipArchive;

    // Helper: open the resulting zip and return a ZipArchive
    fn open_zip(zip_path: &Path) -> ZipArchive<File> {
        let f = File::open(zip_path).expect("zip file should exist");
        ZipArchive::new(f).expect("should be a valid zip archive")
    }

    // Helper: collect all entry names from a zip archive
    fn entry_names(archive: &mut ZipArchive<File>) -> Vec<String> {
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn test_zip_file_is_created() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("myfolder");
        fs::create_dir_all(&folder).unwrap();
        File::create(folder.join("a.txt")).unwrap();

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        assert!(zip_path.exists(), "zip file should be created on disk");
    }

    #[test]
    fn test_single_file_content_preserved() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("myfolder");
        fs::create_dir_all(&folder).unwrap();

        let mut f = File::create(folder.join("hello.txt")).unwrap();
        writeln!(f, "hello world").unwrap();
        drop(f);

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let mut entry = archive
            .by_name("myfolder/hello.txt")
            .expect("entry should exist");
        let mut contents = String::new();
        entry.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "hello world\n");
    }

    #[test]
    fn test_folder_name_is_top_level_entry_in_zip() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("myproject");
        fs::create_dir_all(&folder).unwrap();
        File::create(folder.join("readme.txt")).unwrap();

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let names = entry_names(&mut archive);
        // All entries should be rooted under the folder name
        assert!(
            names.iter().all(|n| n.starts_with("myproject")),
            "all entries should be under 'myproject/', got: {:?}",
            names
        );
    }

    #[test]
    fn test_nested_directory_structure_preserved() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("root");
        let sub = folder.join("subdir");
        fs::create_dir_all(&sub).unwrap();

        let mut f = File::create(sub.join("nested.txt")).unwrap();
        write!(f, "nested content").unwrap();
        drop(f);

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let names = entry_names(&mut archive);

        // Expect a directory entry for subdir and a file entry for the nested file
        assert!(
            names.contains(&"root/subdir/".to_string()),
            "subdir directory entry expected, got: {:?}",
            names
        );
        assert!(
            names.contains(&"root/subdir/nested.txt".to_string()),
            "nested file entry expected, got: {:?}",
            names
        );
    }

    #[test]
    fn test_multiple_files_all_present_in_zip() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("multi");
        fs::create_dir_all(&folder).unwrap();

        for name in &["a.txt", "b.txt", "c.txt"] {
            let mut f = File::create(folder.join(name)).unwrap();
            write!(f, "content of {}", name).unwrap();
        }

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let names = entry_names(&mut archive);

        for name in &["a.txt", "b.txt", "c.txt"] {
            assert!(
                names.contains(&format!("multi/{}", name)),
                "'multi/{}' not found in zip, entries: {:?}",
                name,
                names
            );
        }
    }

    #[test]
    fn test_multiple_file_contents_correct() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("multi");
        fs::create_dir_all(&folder).unwrap();

        for name in &["a.txt", "b.txt"] {
            let mut f = File::create(folder.join(name)).unwrap();
            write!(f, "data:{}", name).unwrap();
        }

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        for name in &["a.txt", "b.txt"] {
            let entry_name = format!("multi/{}", name);
            let mut entry = archive.by_name(&entry_name).unwrap();
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, format!("data:{}", name));
        }
    }

    #[test]
    fn test_empty_folder_produces_valid_zip() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("empty");
        fs::create_dir_all(&folder).unwrap();

        let zip_path = dir.path().join("out.zip");
        compress_folder(&folder, &zip_path).unwrap();

        // Zip should exist and be openable even if the folder has no files
        assert!(zip_path.exists());
        let mut archive = open_zip(&zip_path);
        // No file entries expected for an empty folder (root itself is skipped)
        let names = entry_names(&mut archive);
        assert!(
            names.iter().all(|n| !n.contains(".txt")),
            "no file entries expected: {:?}",
            names
        );
    }

    #[test]
    fn test_large_file_content_preserved_via_bufreader() {
        // Regression/boundary: verifies BufReader + std::io::copy correctly streams large data
        let dir = tempdir().unwrap();
        let folder = dir.path().join("bigfolder");
        fs::create_dir_all(&folder).unwrap();

        // Write ~512 KB of repeated data
        let chunk = b"ABCDEFGHIJ".repeat(1024); // 10 KB
        let big_data: Vec<u8> = chunk.repeat(50); // ~500 KB
        let file_path = folder.join("big.bin");
        {
            let mut f = File::create(&file_path).unwrap();
            f.write_all(&big_data).unwrap();
        }

        let zip_path = dir.path().join("big.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let mut entry = archive.by_name("bigfolder/big.bin").unwrap();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        assert_eq!(
            buf, big_data,
            "large file content should round-trip correctly"
        );
    }

    #[test]
    fn test_binary_file_content_preserved() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("binfolder");
        fs::create_dir_all(&folder).unwrap();

        // Binary content with all byte values
        let binary_data: Vec<u8> = (0u8..=255).collect();
        {
            let mut f = File::create(folder.join("data.bin")).unwrap();
            f.write_all(&binary_data).unwrap();
        }

        let zip_path = dir.path().join("bin.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let mut entry = archive.by_name("binfolder/data.bin").unwrap();
        let mut result = Vec::new();
        entry.read_to_end(&mut result).unwrap();
        assert_eq!(
            result, binary_data,
            "binary content should be preserved exactly"
        );
    }

    #[test]
    fn test_error_on_invalid_zip_path() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir_all(&folder).unwrap();

        // Point zip_path under a non-existent child directory so File::create fails.
        let bad_parent = dir.path().join("nonexistent_child");
        let bad_zip_path = bad_parent.join("archive.zip");
        let result = compress_folder(&folder, &bad_zip_path);
        assert!(
            result.is_err(),
            "should return an error for invalid zip path"
        );
    }

    #[test]
    fn test_deeply_nested_structure() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("deep");
        let deep = folder.join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();

        let mut f = File::create(deep.join("leaf.txt")).unwrap();
        write!(f, "leaf").unwrap();
        drop(f);

        let zip_path = dir.path().join("deep.zip");
        compress_folder(&folder, &zip_path).unwrap();

        let mut archive = open_zip(&zip_path);
        let names = entry_names(&mut archive);

        assert!(
            names.contains(&"deep/a/b/c/leaf.txt".to_string()),
            "deeply nested file should be present, got: {:?}",
            names
        );
        // All intermediate directory entries should exist
        for dir_entry in &["deep/a/", "deep/a/b/", "deep/a/b/c/"] {
            assert!(
                names.contains(&dir_entry.to_string()),
                "directory entry '{}' should be present, got: {:?}",
                dir_entry,
                names
            );
        }
    }
}
