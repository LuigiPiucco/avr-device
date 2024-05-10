use std::{
    collections::{BTreeMap, HashSet},
    env, error,
    ffi::OsString,
    fmt::Display,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};
use svd2rust::config::IdentFormats;
use svd_rs::Device;
use svdtools::common::svd_reader;

fn main() {
    if let Err(e) = real_main() {
        println!("cargo::error={}", e);
    }
}

fn real_main() -> Result<(), Error> {
    let crate_root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or(Error::top_level(
        ErrorKind::DataMissing("env variable CARGO_MANIFEST_DIR".into()),
    ))?);
    let available_mcu_inputs = InputFinder::find(&crate_root)?;

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or(Error::top_level(
        ErrorKind::DataMissing("env variable OUT_DIR".into()),
    ))?);

    let svd_generator = SvdGenerator::from_out_dir(&out_dir)?;
    let code_generator = CodeGenerator::from_out_dir(&out_dir)?;

    let mcu_inputs = available_mcu_inputs.filter_from_features()?;
    let mut svds = BTreeMap::new();
    for (mcu, inputs) in mcu_inputs.get_map() {
        let _ = svds.insert(mcu.as_str(), svd_generator.generate(mcu, inputs)?);
    }
    for (mcu, svd) in &svds {
        code_generator.generate_module(mcu, svd)?;
    }
    code_generator.generate_vector(&svds)?;

    Ok(())
}

#[derive(Debug)]
struct Error {
    pub stage: Stage,
    pub kind: ErrorKind,
}

impl Error {
    pub fn top_level(kind: ErrorKind) -> Self {
        Self {
            stage: Stage::TopLevel,
            kind,
        }
    }
    pub fn directory_creation(kind: ErrorKind) -> Self {
        Self {
            stage: Stage::DirectoryCreation,
            kind,
        }
    }
    pub fn input_finding(kind: ErrorKind) -> Self {
        Self {
            stage: Stage::InputFinding,
            kind,
        }
    }
    pub fn svd_generation(mcu: String, kind: ErrorKind) -> Self {
        Self {
            stage: Stage::SvdGeneration(mcu),
            kind,
        }
    }
    pub fn module_generation(mcu: String, kind: ErrorKind) -> Self {
        Self {
            stage: Stage::ModuleGeneration(mcu),
            kind,
        }
    }
    pub fn vector_generation(kind: ErrorKind) -> Self {
        Self {
            stage: Stage::VectorGeneration,
            kind,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "during {}, error: {}", self.stage, self.kind)
    }
}
impl error::Error for Error {}

#[derive(Debug)]
enum Stage {
    TopLevel,
    DirectoryCreation,
    InputFinding,
    SvdGeneration(String),
    ModuleGeneration(String),
    VectorGeneration,
}

impl Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stage::TopLevel => write!(f, "top-level"),
            Stage::DirectoryCreation => write!(f, "output directory creation"),
            Stage::InputFinding => write!(f, "input finding"),
            Stage::SvdGeneration(mcu) => write!(f, "svd generation for {}", mcu),
            Stage::ModuleGeneration(mcu) => write!(f, "module generation for {}", mcu),
            Stage::VectorGeneration => write!(f, "vector generation"),
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    Io(PathBuf, io::Error),
    DataMissing(String),
    DataInvalid(String),
    NoMcuSelected(Vec<String>),
    ParseAtdf(String),
    Atdf2Svd(String),
    ParseYamlPatch(anyhow::Error),
    ProcessSvd(anyhow::Error),
    ReadSvd(anyhow::Error),
    Svd2Rust(anyhow::Error),
    ParseSyn(syn::Error),
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Io(p, e) => write!(f, "{}: {}", p.display(), e),
            ErrorKind::ParseAtdf(msg) => write!(f, "(parsing atdf) {}", msg),
            ErrorKind::Atdf2Svd(msg) => write!(f, "(generating svd) {}", msg),
            ErrorKind::ParseYamlPatch(e) => write!(f, "(parsing yaml) {}", e),
            ErrorKind::ProcessSvd(e) => write!(f, "(patching svd) {}", e),
            ErrorKind::ReadSvd(e) => write!(f, "(reading svd) {}", e),
            ErrorKind::Svd2Rust(e) => write!(f, "(parsing yaml) {}", e),
            ErrorKind::ParseSyn(e) => write!(f, "(parsing rust) {}", e),
            ErrorKind::DataMissing(what) => write!(f, "{} is missing", what),
            ErrorKind::DataInvalid(what) => write!(f, "{} is invalid", what),
            ErrorKind::NoMcuSelected(items) => write!(
                f,
                "at least one MCU feature must be selected; choose from: {}",
                items.join(", ")
            ),
        }
    }
}
impl From<Result<ErrorKind, ErrorKind>> for ErrorKind {
    fn from(value: Result<ErrorKind, ErrorKind>) -> Self {
        match value {
            Ok(e) => e,
            Err(e) => e,
        }
    }
}

#[derive(Debug)]
struct McuInputs {
    atdf_path: PathBuf,
    atdf: atdf2svd::chip::Chip,
    patch: Option<yaml_rust2::Yaml>,
}

#[derive(Debug)]
struct InputFinder {
    inputs: BTreeMap<String, McuInputs>,
}

impl InputFinder {
    pub fn get_map(&self) -> &BTreeMap<String, McuInputs> {
        &self.inputs
    }

    pub fn filter_from_features(self) -> Result<Self, Error> {
        let (selected_mcus, other_mcus) =
            self.inputs
                .into_iter()
                .partition::<BTreeMap<_, _>, _>(|(mcu, _)| {
                    env::var_os(format!("CARGO_FEATURE_{}", mcu.to_uppercase())).is_some()
                });
        if !selected_mcus.is_empty() {
            Ok(Self {
                inputs: selected_mcus,
            })
        } else {
            // In this case `other_mcus` contains all of them, for listing when
            // printing the error.
            Err(Error::top_level(ErrorKind::NoMcuSelected(
                other_mcus.into_keys().collect::<Vec<_>>(),
            )))
        }
    }

    pub fn find(crate_root: &Path) -> Result<Self, Error> {
        let packs_dir = crate_root.join("vendor");
        let patches_dir = crate_root.join("patch");

        Self::track_path(&packs_dir)?;
        Self::track_path(&patches_dir)?;

        let mut inputs = BTreeMap::new();
        for result in fs::read_dir(&packs_dir)
            .map_err(|e| Error::input_finding(ErrorKind::Io(packs_dir.to_owned(), e)))?
        {
            let entry =
                result.map_err(|e| Error::input_finding(ErrorKind::Io(packs_dir.to_owned(), e)))?;
            let atdf_path = entry.path();
            if atdf_path.extension() != Some(&OsString::from("atdf")) {
                continue;
            }
            let mcu_name = atdf_path
                .file_stem()
                .ok_or(Error::input_finding(ErrorKind::DataInvalid(format!(
                    "file with no stem in `{}`",
                    atdf_path.display()
                ))))?
                .to_str()
                .ok_or(Error::input_finding(ErrorKind::DataInvalid(format!(
                    "file with non UTF-8 name in `{}`",
                    atdf_path.display()
                ))))?
                .to_owned();

            let atdf_file = File::open(&atdf_path)
                .map_err(|e| Error::input_finding(ErrorKind::Io(atdf_path.to_owned(), e)))?;
            let atdf = atdf2svd::atdf::parse(atdf_file, &mut HashSet::new()).map_err(|e| {
                Error::input_finding(
                    atdf_error(&atdf_path, e)
                        .map(|e| ErrorKind::ParseAtdf(e))
                        .into(),
                )
            })?;
            let patch_path = patches_dir.join(&mcu_name).with_extension("yaml");
            let patch = if patch_path
                .try_exists()
                .map_err(|e| Error::input_finding(ErrorKind::Io(patch_path.to_owned(), e)))?
            {
                Some(
                    svdtools::patch::load_patch(&patch_path)
                        .map_err(|e| Error::input_finding(ErrorKind::ParseYamlPatch(e)))?,
                )
            } else {
                None
            };
            inputs.insert(
                mcu_name,
                McuInputs {
                    atdf_path,
                    atdf,
                    patch,
                },
            );
        }

        Ok(Self { inputs })
    }

    fn track_path(path: &Path) -> Result<(), Error> {
        if !path
            .try_exists()
            .map_err(|e| Error::input_finding(ErrorKind::Io(path.to_owned(), e)))?
        {
            return Err(Error::input_finding(ErrorKind::DataMissing(format!(
                "required path `{}`",
                path.display()
            ))));
        } else if path.is_symlink() {
            return Err(Error::input_finding(ErrorKind::DataInvalid(format!(
                "required path `{}` being a symlink",
                path.display()
            ))));
        }

        if path.is_dir() {
            for result in fs::read_dir(path)
                .map_err(|e| Error::input_finding(ErrorKind::Io(path.to_owned(), e)))?
            {
                let subpath = result
                    .map_err(|e| Error::input_finding(ErrorKind::Io(path.to_owned(), e)))?
                    .path();
                Self::track_path(&subpath)?;
            }
        }

        println!("cargo::rerun-if-changed={}", path.display());

        Ok(())
    }
}

#[derive(Debug)]
struct SvdGenerator {
    unpatched_svd: PathBuf,
    patched_svd: PathBuf,
}

impl SvdGenerator {
    pub fn from_out_dir(out_dir: &Path) -> Result<Self, Error> {
        let svd_dir = out_dir.join("svd");
        let unpatched_svd = svd_dir.join("unpatched");
        let patched_svd = svd_dir.join("patched");

        ensure_out_dir(&unpatched_svd)?;
        ensure_out_dir(&patched_svd)?;

        Ok(Self {
            unpatched_svd,
            patched_svd,
        })
    }

    pub fn generate(&self, mcu: &str, inputs: &McuInputs) -> Result<Device, Error> {
        let unpatched_svd_path = self.unpatched_svd.join(mcu).with_extension("svd");
        let unpatched_writer = File::create(&unpatched_svd_path).map_err(|e| {
            Error::svd_generation(mcu.into(), ErrorKind::Io(unpatched_svd_path.to_owned(), e))
        })?;
        atdf2svd::svd::generate(&inputs.atdf, &unpatched_writer).map_err(|e| {
            Error::svd_generation(
                mcu.into(),
                atdf_error(&inputs.atdf_path, e)
                    .map(|e| ErrorKind::Atdf2Svd(e))
                    .into(),
            )
        })?;
        let unpatched_reader = File::open(&unpatched_svd_path).map_err(|e| {
            Error::svd_generation(mcu.into(), ErrorKind::Io(unpatched_svd_path.to_owned(), e))
        })?;
        let patched_svd_path = self.patched_svd.join(mcu).with_extension("svd");
        let svd_path = if let Some(patch) = &inputs.patch {
            let mut reader = svdtools::patch::process_reader(
                unpatched_reader,
                &patch,
                &Default::default(),
                &Default::default(),
            )
            .map_err(|e| Error::svd_generation(mcu.into(), ErrorKind::ProcessSvd(e)))?;
            let mut b = Vec::new();
            reader.read_to_end(&mut b).map_err(|e| {
                Error::svd_generation(mcu.into(), ErrorKind::Io(unpatched_svd_path.to_owned(), e))
            })?;

            fs::write(&patched_svd_path, b).map_err(|e| {
                Error::svd_generation(mcu.into(), ErrorKind::Io(patched_svd_path.to_owned(), e))
            })?;
            patched_svd_path
        } else {
            unpatched_svd_path
        };

        svd_reader::device(&svd_path)
            .map_err(|e| Error::svd_generation(mcu.into(), ErrorKind::ReadSvd(e)))
    }
}

#[derive(Debug)]
struct CodeGenerator {
    module: PathBuf,
}

impl CodeGenerator {
    pub fn generate_module(&self, mcu: &str, svd: &Device) -> Result<(), Error> {
        let mut svd2rust_config = svd2rust::Config::default();
        svd2rust_config.target = svd2rust::Target::None;
        svd2rust_config.generic_mod = true;
        svd2rust_config.make_mod = true;
        svd2rust_config.strict = true;
        svd2rust_config.output_dir = Some(self.module.clone());
        svd2rust_config.skip_crate_attributes = true;
        svd2rust_config.reexport_core_peripherals = false;
        svd2rust_config.reexport_interrupt = false;
        svd2rust_config.ident_formats = IdentFormats::default_theme();

        let generated_stream =
            svd2rust::generate::device::render(&svd, &svd2rust_config, &mut String::new())
                .map_err(|e| Error::module_generation(mcu.into(), ErrorKind::Svd2Rust(e)))?;

        let mut syntax_tree: syn::File = syn::parse2(generated_stream)
            .map_err(|e| Error::module_generation(mcu.into(), ErrorKind::ParseSyn(e)))?;
        for item in syntax_tree.items.iter_mut() {
            {
                let syn::Item::Static(statik) = item else {
                    continue;
                };
                if &statik.ident.to_string() != "DEVICE_PERIPHERALS" {
                    continue;
                }
            }
            *item = syn::parse_quote! {use super::DEVICE_PERIPHERALS;};
            break;
        }

        let formatted = prettyplease::unparse(&syntax_tree);

        let module_path = self.module.join(mcu).with_extension("rs");
        fs::write(&module_path, &formatted).map_err(|e| {
            Error::module_generation(mcu.into(), ErrorKind::Io(module_path.to_owned(), e))
        })
    }

    fn generate_vector(self, devices: &BTreeMap<&str, Device>) -> Result<(), Error> {
        let mut specific_matchers = Vec::new();
        for (mcu, device) in devices {
            for p in &device.peripherals {
                for i in &p.interrupt {
                    specific_matchers.push(format!(
                        r#"
            (@{0}, {1}, $it:item) => {{
                #[export_name = "__vector_{2}"]
                $it
            }};"#,
                        mcu, i.name, i.value,
                    ));
                }
            }
        }

        let content = format!(
            r#"#[doc(hidden)]
#[macro_export]
macro_rules! __avr_device_trampoline {{
    {}
    (@$mcu:ident, $name:ident, $it:item) => {{
        compile_error!(concat!("Couldn't find interrupt ", stringify!($name), ", for MCU ", stringify!($mcu), "."));
    }}
}}
"#,
            specific_matchers.concat(),
        );
        let vector_path = self.module.join("vector.rs");
        fs::write(&vector_path, content)
            .map_err(|e| Error::vector_generation(ErrorKind::Io(vector_path.to_owned(), e)))
    }

    pub fn from_out_dir(out_dir: &Path) -> Result<Self, Error> {
        let module = out_dir.join("pac");
        ensure_out_dir(&module)?;
        Ok(Self { module })
    }
}

fn ensure_out_dir(dir: &Path) -> Result<(), Error> {
    if dir.is_file() || dir.is_symlink() {
        return Err(Error::directory_creation(ErrorKind::DataInvalid(format!(
            "path `{}` being a non-directory",
            dir.display()
        ))));
    }
    if !dir.is_dir() {
        fs::create_dir_all(dir)
            .map_err(|e| Error::directory_creation(ErrorKind::Io(dir.to_owned(), e)))?;
    }
    Ok(())
}

fn atdf_error(path: &Path, e: atdf2svd::Error) -> Result<String, ErrorKind> {
    let mut v = Vec::new();
    if let Err(e) = e.format(&mut v) {
        return Err(ErrorKind::Io(path.to_owned(), e));
    }
    let s = v.utf8_chunks().fold(String::new(), |mut s, chunk| {
        s.push_str(chunk.valid());
        s
    });
    Ok(s)
}
