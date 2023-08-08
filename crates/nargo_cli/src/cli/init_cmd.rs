use crate::errors::CliError;

use super::fs::{create_named_dir, write_to_file};
use super::{NargoConfig, CARGO_PKG_VERSION};
use acvm::Backend;
use clap::Args;
use nargo::constants::{PKG_FILE, SRC_DIR};
use nargo::package::PackageType;
use std::path::PathBuf;

/// Create a Noir project in the current directory.
#[derive(Debug, Clone, Args)]
pub(crate) struct InitCommand {
    /// Name of the package [default: current directory name]
    #[clap(long)]
    name: Option<String>,

    /// Use a library template
    #[arg(long, conflicts_with = "bin", conflicts_with = "contract")]
    pub(crate) lib: bool,

    /// Use a binary template [default]
    #[arg(long, conflicts_with = "lib", conflicts_with = "contract")]
    pub(crate) bin: bool,

    /// Use a contract template
    #[arg(long, conflicts_with = "lib", conflicts_with = "bin")]
    pub(crate) contract: bool,
}

const BIN_EXAMPLE: &str = r#"fn main(x : Field, y : pub Field) {
    assert(x != y);
}

#[test]
fn test_main() {
    main(1, 2);

    // Uncomment to make test fail
    // main(1, 1);
}
"#;

const CONTRACT_EXAMPLE: &str = r#"contract Main {
    internal fn double(x: Field) -> pub Field { x * 2 }
    fn triple(x: Field) -> pub Field { x * 3 }
    fn quadruple(x: Field) -> pub Field { double(double(x)) }
}
"#;

const LIB_EXAMPLE: &str = r#"fn my_util(x : Field, y : Field) -> bool {
    x != y
}

#[test]
fn test_my_util() {
    assert(my_util(1, 2));

    // Uncomment to make test fail
    // assert(my_util(1, 1));
}
"#;

pub(crate) fn run<B: Backend>(
    // Backend is currently unused, but we might want to use it to inform the "new" template in the future
    _backend: &B,
    args: InitCommand,
    config: NargoConfig,
) -> Result<(), CliError<B>> {
    let package_name = args
        .name
        .unwrap_or_else(|| config.program_dir.file_name().unwrap().to_str().unwrap().to_owned());

    let package_type = if args.lib {
        PackageType::Library
    } else if args.contract {
        PackageType::Contract
    } else {
        PackageType::Binary
    };
    initialize_project(config.program_dir, &package_name, package_type);
    Ok(())
}

/// Initializes a new Noir project in `package_dir`.
pub(crate) fn initialize_project(
    package_dir: PathBuf,
    package_name: &str,
    package_type: PackageType,
) {
    let src_dir = package_dir.join(SRC_DIR);
    create_named_dir(&src_dir, "src");

    let toml_contents = format!(
        r#"[package]
name = "{package_name}"
type = "{package_type}"
authors = [""]
compiler_version = "{CARGO_PKG_VERSION}"

[dependencies]"#
    );

    write_to_file(toml_contents.as_bytes(), &package_dir.join(PKG_FILE));
    // This uses the `match` syntax instead of `if` so we get a compile error when we add new package types (which likely need new template files)
    match package_type {
        PackageType::Binary => write_to_file(BIN_EXAMPLE.as_bytes(), &src_dir.join("main.nr")),
        PackageType::Contract => {
            write_to_file(CONTRACT_EXAMPLE.as_bytes(), &src_dir.join("main.nr"))
        }
        PackageType::Library => write_to_file(LIB_EXAMPLE.as_bytes(), &src_dir.join("lib.nr")),
    };
    println!("Project successfully created! It is located at {}", package_dir.display());
}
