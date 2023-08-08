use acvm::acir::circuit::OpcodeLabel;
use acvm::{acir::circuit::Circuit, Backend};
use iter_extended::try_vecmap;
use iter_extended::vecmap;
use nargo::package::Package;
use nargo::{artifacts::contract::PreprocessedContract, NargoError};
use noirc_driver::{
    compile_contracts, compile_main, CompileOptions, CompiledProgram, ErrorsAndWarnings, Warnings,
};
use noirc_frontend::graph::CrateName;
use noirc_frontend::hir::Context;

use clap::Args;

use nargo::ops::{preprocess_contract_function, preprocess_program};

use crate::errors::{CliError, CompileError};
use crate::manifest::resolve_workspace_from_toml;
use crate::{find_package_manifest, prepare_package};

use super::fs::{
    common_reference_string::{
        read_cached_common_reference_string, update_common_reference_string,
        write_cached_common_reference_string,
    },
    program::{save_contract_to_file, save_program_to_file},
};
use super::NargoConfig;

// TODO(#1388): pull this from backend.
const BACKEND_IDENTIFIER: &str = "acvm-backend-barretenberg";

/// Compile the program and its secret execution trace into ACIR format
#[derive(Debug, Clone, Args)]
pub(crate) struct CompileCommand {
    /// Include Proving and Verification keys in the build artifacts.
    #[arg(long)]
    include_keys: bool,

    /// The name of the package to compile
    #[clap(long)]
    package: Option<CrateName>,

    #[clap(flatten)]
    compile_options: CompileOptions,
}

pub(crate) fn run<B: Backend>(
    backend: &B,
    args: CompileCommand,
    config: NargoConfig,
) -> Result<(), CliError<B>> {
    let toml_path = find_package_manifest(&config.program_dir)?;
    let workspace = resolve_workspace_from_toml(&toml_path, args.package)?;
    let circuit_dir = workspace.target_directory_path();

    let mut common_reference_string = read_cached_common_reference_string();

    for package in &workspace {
        let (mut context, crate_id) = prepare_package(package);
        // If `contract` package type, we're compiling every function in a 'contract' rather than just 'main'.
        if package.is_contract() {
            let result = compile_contracts(&mut context, crate_id, &args.compile_options);
            let contracts = report_errors(result, &context, args.compile_options.deny_warnings)?;

            // TODO(#1389): I wonder if it is incorrect for nargo-core to know anything about contracts.
            // As can be seen here, It seems like a leaky abstraction where ContractFunctions (essentially CompiledPrograms)
            // are compiled via nargo-core and then the PreprocessedContract is constructed here.
            // This is due to EACH function needing it's own CRS, PKey, and VKey from the backend.
            let preprocessed_contracts: Result<Vec<PreprocessedContract>, CliError<B>> =
                try_vecmap(contracts, |contract| {
                    let preprocessed_contract_functions =
                        try_vecmap(contract.functions, |mut func| {
                            func.bytecode = optimize_circuit(backend, func.bytecode)?.0;
                            common_reference_string = update_common_reference_string(
                                backend,
                                &common_reference_string,
                                &func.bytecode,
                            )
                            .map_err(CliError::CommonReferenceStringError)?;

                            preprocess_contract_function(
                                backend,
                                args.include_keys,
                                &common_reference_string,
                                func,
                            )
                            .map_err(CliError::ProofSystemCompilerError)
                        })?;

                    Ok(PreprocessedContract {
                        name: contract.name,
                        backend: String::from(BACKEND_IDENTIFIER),
                        functions: preprocessed_contract_functions,
                    })
                });
            for contract in preprocessed_contracts? {
                save_contract_to_file(
                    &contract,
                    &format!("{}-{}", package.name, contract.name),
                    &circuit_dir,
                );
            }
        } else {
            let (_, program) = compile_package(backend, package, &args.compile_options)?;

            common_reference_string =
                update_common_reference_string(backend, &common_reference_string, &program.circuit)
                    .map_err(CliError::CommonReferenceStringError)?;

            let (preprocessed_program, _) =
                preprocess_program(backend, args.include_keys, &common_reference_string, program)
                    .map_err(CliError::ProofSystemCompilerError)?;
            save_program_to_file(&preprocessed_program, &package.name, &circuit_dir);
        }
    }

    write_cached_common_reference_string(&common_reference_string);

    Ok(())
}

pub(crate) fn compile_package<B: Backend>(
    backend: &B,
    package: &Package,
    compile_options: &CompileOptions,
) -> Result<(Context, CompiledProgram), CompileError> {
    if package.is_library() {
        return Err(CompileError::LibraryCrate(package.name.clone()));
    }

    let (mut context, crate_id) = prepare_package(package);
    let result = compile_main(&mut context, crate_id, compile_options);
    let mut program = report_errors(result, &context, compile_options.deny_warnings)?;
    // Apply backend specific optimizations.
    let (optimized_circuit, opcode_labels) = optimize_circuit(backend, program.circuit)
        .expect("Backend does not support an opcode that is in the IR");

    // TODO(#2110): Why does this set `program.circuit` to `optimized_circuit` instead of the function taking ownership
    // and requiring we use `optimized_circuit` everywhere after
    program.circuit = optimized_circuit;
    let opcode_ids = vecmap(opcode_labels, |label| match label {
        OpcodeLabel::Unresolved => {
            unreachable!("Compiled circuit opcodes must resolve to some index")
        }
        OpcodeLabel::Resolved(index) => index as usize,
    });
    program.debug.update_acir(opcode_ids);

    Ok((context, program))
}

pub(super) fn optimize_circuit<B: Backend>(
    backend: &B,
    circuit: Circuit,
) -> Result<(Circuit, Vec<OpcodeLabel>), CliError<B>> {
    let result = acvm::compiler::compile(circuit, backend.np_language(), |opcode| {
        backend.supports_opcode(opcode)
    })
    .map_err(|_| NargoError::CompilationError)?;

    Ok(result)
}

/// Helper function for reporting any errors in a Result<(T, Warnings), ErrorsAndWarnings>
/// structure that is commonly used as a return result in this file.
pub(crate) fn report_errors<T>(
    result: Result<(T, Warnings), ErrorsAndWarnings>,
    context: &Context,
    deny_warnings: bool,
) -> Result<T, CompileError> {
    let (t, warnings) = result.map_err(|errors| {
        noirc_errors::reporter::report_all(&context.file_manager, &errors, deny_warnings)
    })?;

    noirc_errors::reporter::report_all(&context.file_manager, &warnings, deny_warnings);
    Ok(t)
}
