use std::collections::BTreeMap;
use std::sync::Mutex;

use axoasset::{toml_edit, LocalAsset, SourceFile};
use camino::{Utf8Path, Utf8PathBuf};
use miette::miette;

use super::command::CommandInfo;
use super::errors::Result;
use super::repo::{App, Repo, TestContext, TestContextLock, ToolsImpl};
pub use snapshot::*;
pub use tools::*;

// installer-specific testing
mod homebrew;
mod npm;
mod powershell;
mod shell;
// utils
mod snapshot;
mod tools;

/// Set this env-var to enable running the installer scripts in temp dirs
///
/// If everything's working right, then no problem.
/// Otherwise MEGA DANGER in messing up your computer.
#[cfg(any(target_family = "unix", target_family = "windows"))]
const ENV_RUIN_ME: &str = "RUIN_MY_COMPUTER_WITH_INSTALLERS";
/// Set this at runtime to override STATIC_DIST_BIN
const ENV_RUNTIME_DIST_BIN: &str = "OVERRIDE_CARGO_BIN_EXE_dist";
const STATIC_DIST_BIN: &str = env!("CARGO_BIN_EXE_dist");
const ROOT_DIR: &str = env!("CARGO_MANIFEST_DIR");
static TOOLS: Mutex<Option<Tools>> = Mutex::new(None);

/// axolotlsay 0.1.0 is a nice simple project with shell+powershell+npm+homebrew+msi installers in its release
pub static AXOLOTLSAY: TestContextLock<Tools> = TestContextLock::new(
    &TOOLS,
    &Repo {
        repo_owner: "axodotdev",
        repo_name: "axolotlsay",
        commit_sha: "470fef1c2e1aecc35b1c8a704960d558906c58ff",
        apps: &[App {
            name: "axolotlsay",
            bins: &["axolotlsay"],
        }],
    },
);
/// akaikatana-repack 0.2.0 has multiple bins!
pub static AKAIKATANA_REPACK: TestContextLock<Tools> = TestContextLock::new(
    &TOOLS,
    &Repo {
        repo_owner: "mistydemeo",
        repo_name: "akaikatana-repack",
        commit_sha: "9516f77ab81b7833e0d66de766ecf802e056f91f",
        apps: &[App {
            name: "akaikatana-repack",
            bins: &["akextract", "akmetadata", "akrepack"],
        }],
    },
);
/// axoasset only has libraries!
pub static AXOASSET: TestContextLock<Tools> = TestContextLock::new(
    &TOOLS,
    &Repo {
        repo_owner: "axodotdev",
        repo_name: "axoasset",
        commit_sha: "5d6a531428fb645bbb1259fd401575c6c651be94",
        apps: &[App {
            name: "axoasset",
            bins: &[],
        }],
    },
);
/// generic workspace containing axolotlsay-js and axolotlsay (Rust)
pub static AXOLOTLSAY_HYBRID: TestContextLock<Tools> = TestContextLock::new(
    &TOOLS,
    &Repo {
        repo_owner: "axodotdev",
        repo_name: "axolotlsay-hybrid",
        commit_sha: "bf2bab37685af43ca58e7ee3683dd59db0b7b645",
        apps: &[
            App {
                name: "axolotlsay-js",
                bins: &["axolotlsay-js"],
            },
            App {
                name: "axolotlsay",
                bins: &["axolotlsay"],
            },
        ],
    },
);
pub struct DistResult {
    test_name: String,
    apps: Vec<AppResult>,
}

pub struct AppResult {
    test_name: String,
    trust_hashes: bool,
    app_name: String,
    bins: Vec<String>,
    shell_installer_path: Option<Utf8PathBuf>,
    homebrew_installer_path: Option<Utf8PathBuf>,
    homebrew_skip_install: bool,
    powershell_installer_path: Option<Utf8PathBuf>,
    npm_installer_package_path: Option<Utf8PathBuf>,
    unified_checksum_path: Option<Utf8PathBuf>,
}

pub struct PlanResult {
    test_name: String,
    raw_json: String,
}

pub struct GenerateResult {
    test_name: String,
    github_ci_path: Option<Utf8PathBuf>,
    wxs_path: Option<Utf8PathBuf>,
}

pub struct BuildAndPlanResult {
    build: DistResult,
    plan: PlanResult,
}

impl<'a> TestContext<'a, Tools> {
    /// Run 'dist build -alies --no-local-paths --output-format=json' and return paths to various files that were generated
    pub fn cargo_dist_build_lies(&self, test_name: &str) -> Result<BuildAndPlanResult> {
        // If the dist target dir exists, delete it to avoid cross-contamination
        let out_path = Utf8Path::new("target/distrib/");
        if out_path.exists() {
            LocalAsset::remove_dir_all(out_path)?;
        }

        // build installers
        eprintln!("running dist build -aglobal...");
        let output = self.tools.cargo_dist.output_checked(|cmd| {
            cmd.arg("dist")
                .arg("build")
                .arg("-alies")
                .arg("--no-local-paths")
                .arg("--output-format=json")
        })?;

        let build = self.load_dist_results(test_name, false)?;

        let raw_json = String::from_utf8(output.stdout).expect("plan wasn't utf8!?");
        let plan = PlanResult {
            test_name: test_name.to_owned(),
            raw_json,
        };

        Ok(BuildAndPlanResult { build, plan })
    }

    /// Run `cargo_dist_plan` and `cargo_dist_build_global`
    pub fn cargo_dist_build_and_plan(&self, test_name: &str) -> Result<BuildAndPlanResult> {
        let build = self.cargo_dist_build_global(test_name)?;
        let plan = self.cargo_dist_plan(test_name)?;

        Ok(BuildAndPlanResult { build, plan })
    }

    /// Run 'dist plan --output-format=json' and return dist-manifest.json
    pub fn cargo_dist_plan(&self, test_name: &str) -> Result<PlanResult> {
        let output = self
            .tools
            .cargo_dist
            .output_checked(|cmd| cmd.arg("dist").arg("plan").arg("--output-format=json"))?;
        let raw_json = String::from_utf8(output.stdout).expect("plan wasn't utf8!?");

        Ok(PlanResult {
            test_name: test_name.to_owned(),
            raw_json,
        })
    }
    /// Run 'dist build -aglobal' and return paths to various files that were generated
    pub fn cargo_dist_build_global(&self, test_name: &str) -> Result<DistResult> {
        // If the dist target dir exists, delete it to avoid cross-contamination
        let out_path = Utf8Path::new("target/distrib/");
        if out_path.exists() {
            LocalAsset::remove_dir_all(out_path)?;
        }

        // build installers
        eprintln!("running dist build -aglobal...");
        self.tools
            .cargo_dist
            .output_checked(|cmd| cmd.arg("dist").arg("build").arg("-aglobal"))?;

        self.load_dist_results(test_name, true)
    }

    /// Run 'dist generate' and return paths to various files that were generated
    pub fn cargo_dist_generate(&self, test_name: &str) -> Result<GenerateResult> {
        self.cargo_dist_generate_prefixed(test_name, "")
    }
    /// Run 'dist generate' and return paths to various files that were generated
    /// (also apply a prefix to the github filename)
    pub fn cargo_dist_generate_prefixed(
        &self,
        test_name: &str,
        prefix: &str,
    ) -> Result<GenerateResult> {
        let ci_file_name = format!("{prefix}release.yml");
        let github_ci_path = Utf8Path::new(".github/workflows/").join(ci_file_name);
        let wxs_path = Utf8Path::new("wix/main.wxs").to_owned();
        // Delete files if they already exist
        if github_ci_path.exists() {
            LocalAsset::remove_file(&github_ci_path)?;
        }
        if wxs_path.exists() {
            LocalAsset::remove_file(&wxs_path)?;
        }

        // run generate
        eprintln!("running dist generate...");
        self.tools
            .cargo_dist
            .output_checked(|cmd| cmd.arg("dist").arg("generate"))?;

        Ok(GenerateResult {
            test_name: test_name.to_owned(),
            github_ci_path: github_ci_path.exists().then_some(github_ci_path),
            wxs_path: wxs_path.exists().then_some(wxs_path),
        })
    }

    fn load_dist_results(&self, test_name: &str, trust_hashes: bool) -> Result<DistResult> {
        // read/analyze installers
        eprintln!("loading results...");
        let mut app_results = vec![];
        for app in self.repo.apps {
            let app_name = app.name.to_owned();
            let target_dir = Utf8PathBuf::from("target/distrib");
            let ps_installer = Utf8PathBuf::from(format!("{target_dir}/{app_name}-installer.ps1"));
            let sh_installer = Utf8PathBuf::from(format!("{target_dir}/{app_name}-installer.sh"));
            let brew_app_name = self.options.homebrew_package_name(&app_name);
            let homebrew_skip_install = self.options.homebrew_skip_install(&app_name);
            let homebrew_installer = Utf8PathBuf::from(format!("{target_dir}/{brew_app_name}.rb"));

            let npm_installer =
                Utf8PathBuf::from(format!("{target_dir}/{app_name}-npm-package.tar.gz"));
            let unified_checksum_path = Utf8PathBuf::from(format!("{target_dir}/sha256.sum"));
            app_results.push(AppResult {
                test_name: test_name.to_owned(),
                trust_hashes,
                app_name,
                bins: app.bins.iter().map(|s| s.to_string()).collect(),
                shell_installer_path: sh_installer.exists().then_some(sh_installer),
                powershell_installer_path: ps_installer.exists().then_some(ps_installer),
                homebrew_installer_path: homebrew_installer.exists().then_some(homebrew_installer),
                homebrew_skip_install,
                npm_installer_package_path: npm_installer.exists().then_some(npm_installer),
                unified_checksum_path: unified_checksum_path
                    .exists()
                    .then_some(unified_checksum_path),
            })
        }

        Ok(DistResult {
            test_name: test_name.to_owned(),
            apps: app_results,
        })
    }

    pub fn patch_cargo_toml(&self, new_toml: String) -> Result<()> {
        eprintln!("loading Cargo.toml...");
        let toml_src = axoasset::SourceFile::load_local("Cargo.toml")?;
        let mut toml = toml_src.deserialize_toml_edit()?;
        eprintln!("editing Cargo.toml...");
        let new_table_src = axoasset::SourceFile::new("new-Cargo.toml", new_toml);
        let new_table = new_table_src.deserialize_toml_edit()?;

        // Written slightly verbosely to make it easier to isolate which failed
        let namespaces = ["workspace", "package"];
        for namespace in namespaces {
            let Some(new_meta) = new_table.get(namespace).and_then(|t| t.get("metadata")) else {
                continue;
            };
            let old_namespace = toml[namespace].or_insert(toml_edit::table());
            let old_meta = old_namespace["metadata"].or_insert(toml_edit::table());
            eprintln!("{new_table}");
            for (key, new) in new_meta.as_table().unwrap() {
                let old = &mut old_meta[key];
                *old = new.clone();
            }
        }

        let toml_out = toml.to_string();
        eprintln!("new toml:\n{toml_out}");
        eprintln!("writing Cargo.toml...");
        axoasset::LocalAsset::write_new(&toml_out, "Cargo.toml")?;

        Ok(())
    }

    pub fn patch_dist_workspace(&self, new_toml: String) -> Result<()> {
        eprintln!("loading dist-workspace.toml...");
        let toml_src = axoasset::SourceFile::load_local("dist-workspace.toml")?;
        let mut toml = toml_src.deserialize_toml_edit()?;
        eprintln!("editing dist-workspace.toml...");
        let new_table_src = axoasset::SourceFile::new("new-dist-workspace.toml", new_toml);
        let new_table = new_table_src.deserialize_toml_edit()?;

        if let Some(new_meta) = new_table.get("dist") {
            let old_meta = toml["dist"].or_insert(toml_edit::table());
            eprintln!("{new_table}");
            for (key, new) in new_meta.as_table().unwrap() {
                let old = &mut old_meta[key];
                *old = new.clone();
            }
        }

        let toml_out = toml.to_string();
        eprintln!("writing dist-workspace.toml...");
        axoasset::LocalAsset::write_new(&toml_out, "dist-workspace.toml")?;

        Ok(())
    }

    /// Shim a file into the git repository
    pub fn workspace_write_file(
        &self,
        dest_path: impl AsRef<Utf8Path>,
        contents: impl AsRef<str>,
    ) -> Result<()> {
        axoasset::LocalAsset::write_new(contents.as_ref(), dest_path.as_ref())?;
        eprintln!("wrote file to {}", dest_path.as_ref());
        Ok(())
    }
}

impl DistResult {
    /// check_all but for when you don't expect the installers to run properly (due to hosting)
    pub fn check_all_no_ruin(
        &self,
        ctx: &TestContext<Tools>,
        _expected_bin_dir: &str,
    ) -> Result<Snapshots> {
        self.linttests(ctx)?;
        // Now that all other checks have passed, it's safe to check snapshots
        self.snapshot()
    }

    pub fn check_all(&self, ctx: &TestContext<Tools>, expected_bin_dir: &str) -> Result<Snapshots> {
        self.linttests(ctx)?;
        self.runtests(ctx, expected_bin_dir)?;
        // Now that all other checks have passed, it's safe to check snapshots
        self.snapshot()
    }

    pub fn linttests(&self, ctx: &TestContext<Tools>) -> Result<()> {
        for app in &self.apps {
            // If we have shellcheck, check our shell script
            app.shellcheck(ctx)?;

            // If we have PsScriptAnalyzer, check our powershell script
            app.psanalyzer(ctx)?;
        }
        Ok(())
    }

    pub fn runtests(&self, ctx: &TestContext<Tools>, expected_bin_dir: &str) -> Result<()> {
        for app in &self.apps {
            // If we can, run the shell script in a temp HOME
            app.runtest_shell_installer(ctx, expected_bin_dir)?;

            // If we can, run the powershell script in a temp HOME
            app.runtest_powershell_installer(ctx, expected_bin_dir)?;

            // If we can, run the homebrew script in a temp HOME
            app.runtest_homebrew_installer(ctx)?;

            // If we can, run the npm package
            app.runtest_npm_installer(ctx)?;
        }
        Ok(())
    }
}

impl AppResult {
    #[cfg(any(target_family = "unix", target_family = "windows"))]
    fn check_install_receipt(
        &self,
        _ctx: &TestContext<Tools>,
        bin_dir: &Utf8Path,
        receipt_file: &Utf8Path,
        bin_ext: &str,
    ) {
        // Check that the install receipt works
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct InstallReceipt {
            binaries: Vec<String>,
            install_prefix: String,
            provider: InstallReceiptProvider,
            source: InstallReceiptSource,
            version: String,
        }
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct InstallReceiptProvider {
            source: String,
            version: String,
        }
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct InstallReceiptSource {
            app_name: String,
            name: String,
            owner: String,
            release_type: String,
        }

        let manifest = if Utf8Path::new("dist-workspace.toml").exists() {
            "dist-workspace.toml"
        } else if Utf8Path::new("dist.toml").exists() {
            "dist.toml"
        } else if Utf8Path::new("Cargo.toml").exists() {
            "Cargo.toml"
        } else {
            panic!(
                "Unable to locate manifest! Checked: dist-workspace.toml, dist.toml, Cargo.toml"
            );
        };

        let toml;
        let metadata = match manifest {
            "Cargo.toml" => {
                toml = axoasset::SourceFile::load_local("Cargo.toml")
                    .unwrap()
                    .deserialize_toml_edit()
                    .unwrap();
                toml.get("workspace")
                    .and_then(|t| t.get("metadata"))
                    .and_then(|t| t.get("dist"))
                    .unwrap()
            }
            "dist-workspace.toml" | "dist.toml" => {
                toml = axoasset::SourceFile::load_local(manifest)
                    .unwrap()
                    .deserialize_toml_edit()
                    .unwrap();
                toml.get("dist").unwrap()
            }
            _ => {
                panic!(
                    "Unable to locate manifest! Checked: dist-workspace.toml, dist.toml, Cargo.toml"
                );
            }
        };

        // If not defined, or if it's one string that equals CARGO_HOME,
        // we have a prefix-style layout and the receipt will specify
        // the parent instead of the bin dir.
        let mut receipt_dir = bin_dir;
        if let Some(install_path) = metadata.get("install-path") {
            if install_path.as_str() == Some("CARGO_HOME") {
                receipt_dir = bin_dir.parent().unwrap();
            }
        } else {
            receipt_dir = bin_dir.parent().unwrap();
        }

        assert!(receipt_file.exists());
        let receipt_src = SourceFile::load_local(receipt_file).expect("couldn't load receipt file");
        let receipt: InstallReceipt = receipt_src.deserialize_json().unwrap();
        assert_eq!(receipt.source.app_name, self.app_name);
        assert_eq!(
            receipt.binaries,
            self.bins
                .iter()
                .map(|s| format!("{s}{bin_ext}"))
                .collect::<Vec<_>>()
        );
        let receipt_bin_dir = receipt
            .install_prefix
            .trim_end_matches('/')
            .trim_end_matches('\\')
            .to_owned();
        let expected_bin_dir = receipt_dir
            .to_string()
            .trim_end_matches('/')
            .trim_end_matches('\\')
            .to_owned();
        assert_eq!(receipt_bin_dir, expected_bin_dir);
    }
}

impl PlanResult {
    pub fn check_all(&self) -> Result<Snapshots> {
        self.parse()?;
        self.snapshot()
    }

    pub fn parse(&self) -> Result<cargo_dist_schema::DistManifest> {
        let src = SourceFile::new("dist-manifest.json", self.raw_json.clone());
        let val = src.deserialize_json()?;
        Ok(val)
    }
}

impl BuildAndPlanResult {
    pub fn check_all_no_ruin(
        &self,
        ctx: &TestContext<Tools>,
        expected_bin_dir: &str,
    ) -> Result<Snapshots> {
        let build_snaps = self.build.check_all_no_ruin(ctx, expected_bin_dir)?;
        let plan_snaps = self.plan.check_all()?;

        // Merge snapshots
        let snaps = build_snaps.join(plan_snaps);
        Ok(snaps)
    }
    pub fn check_all(&self, ctx: &TestContext<Tools>, expected_bin_dir: &str) -> Result<Snapshots> {
        let build_snaps = self.build.check_all(ctx, expected_bin_dir)?;
        let plan_snaps = self.plan.check_all()?;

        // Merge snapshots
        let snaps = build_snaps.join(plan_snaps);
        Ok(snaps)
    }
}

impl GenerateResult {
    pub fn check_all(&self) -> Result<Snapshots> {
        self.snapshot()
    }
}
