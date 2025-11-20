// pyo3 macros use a gil-refs feature
#![allow(unexpected_cfgs)]
use futures::StreamExt;
use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyStopIteration, PyValueError};
use pyo3::import_exception;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};
use std::str::FromStr;
use std::sync::OnceLock;
use upstream_ontologist::{Certainty, Origin};
use url::Url;

import_exception!(urllib.error, HTTPError);

// Global Tokio runtime that's initialized once and reused
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Gets or initializes the global Tokio runtime for async operations.
///
/// Returns a reference to a static Tokio runtime with 2 worker threads.
fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

/// Removes VCS-specific prefixes from URL schemes.
///
/// Converts URLs like "git+https://..." to "https://...".
///
/// Args:
///     url: The URL string to process.
///
/// Returns:
///     The URL with VCS prefixes removed from the scheme.
#[pyfunction]
fn drop_vcs_in_scheme(url: &str) -> String {
    upstream_ontologist::vcs::drop_vcs_in_scheme(&url.parse().unwrap())
        .map_or_else(|| url.to_string(), |u| u.to_string())
}

/// Converts a Git repository URL to its canonical form.
///
/// This function normalizes Git URLs by resolving redirects and applying
/// standard transformations to get the canonical repository location.
///
/// Args:
///     url: The Git repository URL to canonicalize.
///     net_access: Whether to allow network access for checking redirects.
///                 If None, network access is allowed by default.
///
/// Returns:
///     The canonical URL string.
///
/// Raises:
///     RuntimeError: If the URL is invalid.
#[pyfunction]
#[pyo3(signature = (url, net_access=None))]
fn canonical_git_repo_url(url: &str, net_access: Option<bool>) -> PyResult<String> {
    let url =
        Url::parse(url).map_err(|e| PyRuntimeError::new_err(format!("Invalid URL: {}", e)))?;
    let rt = get_runtime();
    Ok(rt
        .block_on(upstream_ontologist::vcs::canonical_git_repo_url(
            &url, net_access,
        ))
        .map_or_else(|| url.to_string(), |u| u.to_string()))
}

/// Attempts to find a public repository URL from a given URL.
///
/// This function tries to convert private or internal repository URLs
/// to their public equivalents.
///
/// Args:
///     url: The repository URL to convert.
///     net_access: Whether to allow network access for verification.
///
/// Returns:
///     The public repository URL if found, None otherwise.
#[pyfunction]
#[pyo3(signature = (url, net_access=None))]
fn find_public_repo_url(url: &str, net_access: Option<bool>) -> PyResult<Option<String>> {
    let rt = get_runtime();
    Ok(rt.block_on(upstream_ontologist::vcs::find_public_repo_url(
        url, net_access,
    )))
}

/// Checks if an upstream datum is a known bad guess.
///
/// Some metadata values are known to be incorrect or low-quality guesses
/// that should be filtered out.
///
/// Args:
///     datum: The UpstreamDatum to check.
///
/// Returns:
///     True if the datum is known to be a bad guess, False otherwise.
#[pyfunction]
fn known_bad_guess(py: Python, datum: Py<PyAny>) -> PyResult<bool> {
    let datum: upstream_ontologist::UpstreamDatum = datum.extract(py)?;
    Ok(datum.known_bad_guess())
}

/// Converts RCP-style Git URLs to standard format.
///
/// Transforms URLs in the format "user@host:path" to "ssh://user@host/path".
///
/// Args:
///     url: The Git URL to fix up.
///
/// Returns:
///     The URL in standard format.
#[pyfunction]
fn fixup_rcp_style_git_repo_url(url: &str) -> PyResult<String> {
    Ok(upstream_ontologist::vcs::fixup_rcp_style_git_repo_url(url)
        .map_or(url.to_string(), |u| u.to_string()))
}

/// Finds a secure (HTTPS) version of a repository URL.
///
/// Attempts to convert HTTP URLs to HTTPS when possible.
///
/// Args:
///     url: The repository URL to secure.
///     branch: Optional branch name to verify.
///     net_access: Whether to allow network access for verification.
///
/// Returns:
///     The secure URL if found, None otherwise.
#[pyfunction]
#[pyo3(signature = (url, branch=None, net_access=None))]
pub fn find_secure_repo_url(
    url: String,
    branch: Option<&str>,
    net_access: Option<bool>,
) -> Option<String> {
    let rt = get_runtime();
    rt.block_on(upstream_ontologist::vcs::find_secure_repo_url(
        url.parse().unwrap(),
        branch,
        net_access,
    ))
    .map(|u| u.to_string())
}

/// Converts a list of CVS repository URLs to a single string representation.
///
/// Args:
///     urls: List of CVS repository URLs.
///
/// Returns:
///     A string representation of the CVS repository if valid, None otherwise.
#[pyfunction]
fn convert_cvs_list_to_str(urls: Vec<String>) -> Option<String> {
    let urls = urls.iter().map(|x| x.as_str()).collect::<Vec<&str>>();
    upstream_ontologist::vcs::convert_cvs_list_to_str(urls.as_slice())
}

/// Fixes broken or malformed Git repository details.
///
/// Attempts to correct common issues with Git repository URLs, branches,
/// and subpaths.
///
/// Args:
///     location: The Git repository URL.
///     branch: Optional branch name.
///     subpath: Optional subpath within the repository.
///
/// Returns:
///     A tuple of (fixed_url, fixed_branch, fixed_subpath).
#[pyfunction]
#[pyo3(signature = (location, branch=None, subpath=None))]
fn fixup_broken_git_details(
    location: &str,
    branch: Option<&str>,
    subpath: Option<&str>,
) -> (String, Option<String>, Option<String>) {
    let rt = get_runtime();
    let url = rt.block_on(upstream_ontologist::vcs::fixup_git_url(location));
    let location = upstream_ontologist::vcs::VcsLocation {
        url: url.parse().unwrap(),
        branch: branch.map(|s| s.to_string()),
        subpath: subpath.map(|s| s.to_string()),
    };
    let ret = rt.block_on(upstream_ontologist::vcs::fixup_git_location(&location));
    (
        ret.url.to_string(),
        ret.branch.as_ref().map(|s| s.to_string()),
        ret.subpath.as_ref().map(|s| s.to_string()),
    )
}

/// Extracts a string value from a Python object.
///
/// Helper function to convert Python objects to Rust strings.
fn extract_str_value(py: Python, value: Py<PyAny>) -> PyResult<String> {
    let value = value.extract::<Py<PyAny>>(py)?;

    value.extract::<String>(py)
}

/// A single piece of upstream project metadata with certainty and origin information.
///
/// Represents metadata fields like Name, Version, Homepage, Repository, etc.
/// along with information about how certain we are of the value and where it came from.
#[derive(Clone)]
#[pyclass]
struct UpstreamDatum(pub(crate) upstream_ontologist::UpstreamDatumWithMetadata);

#[pymethods]
impl UpstreamDatum {
    /// Creates a new UpstreamDatum.
    ///
    /// Args:
    ///     field: The metadata field name (e.g., "Name", "Version", "Homepage").
    ///     value: The value for this field.
    ///     certainty: Optional certainty level (e.g., "certain", "confident", "possible").
    ///     origin: Optional origin information describing where this datum came from.
    ///
    /// Raises:
    ///     ValueError: If the field name is not recognized.
    #[new]
    #[pyo3(signature = (field, value, certainty=None, origin=None))]
    fn new(
        py: Python,
        field: String,
        value: Py<PyAny>,
        certainty: Option<String>,
        origin: Option<Origin>,
    ) -> PyResult<Self> {
        Ok(UpstreamDatum(
            upstream_ontologist::UpstreamDatumWithMetadata {
                datum: match field.as_str() {
                    "Name" => {
                        upstream_ontologist::UpstreamDatum::Name(extract_str_value(py, value)?)
                    }
                    "Version" => {
                        upstream_ontologist::UpstreamDatum::Version(extract_str_value(py, value)?)
                    }
                    "Summary" => {
                        upstream_ontologist::UpstreamDatum::Summary(extract_str_value(py, value)?)
                    }
                    "Description" => upstream_ontologist::UpstreamDatum::Description(
                        extract_str_value(py, value)?,
                    ),
                    "Homepage" => {
                        upstream_ontologist::UpstreamDatum::Homepage(extract_str_value(py, value)?)
                    }
                    "Repository" => {
                        // Check if the value is a list rather than a string
                        if let Ok(value) = value.extract::<Vec<String>>(py) {
                            upstream_ontologist::UpstreamDatum::Repository(value.join(" "))
                        } else {
                            upstream_ontologist::UpstreamDatum::Repository(extract_str_value(
                                py, value,
                            )?)
                        }
                    }
                    "Repository-Browse" => upstream_ontologist::UpstreamDatum::RepositoryBrowse(
                        extract_str_value(py, value)?,
                    ),
                    "License" => {
                        upstream_ontologist::UpstreamDatum::License(extract_str_value(py, value)?)
                    }
                    "Author" => {
                        upstream_ontologist::UpstreamDatum::Author(value.extract(py).unwrap())
                    }
                    "Bug-Database" => upstream_ontologist::UpstreamDatum::BugDatabase(
                        extract_str_value(py, value)?,
                    ),
                    "Bug-Submit" => {
                        upstream_ontologist::UpstreamDatum::BugSubmit(extract_str_value(py, value)?)
                    }
                    "Contact" => {
                        upstream_ontologist::UpstreamDatum::Contact(extract_str_value(py, value)?)
                    }
                    "Cargo-Crate" => upstream_ontologist::UpstreamDatum::CargoCrate(
                        extract_str_value(py, value)?,
                    ),
                    "Security-MD" => upstream_ontologist::UpstreamDatum::SecurityMD(
                        extract_str_value(py, value)?,
                    ),
                    "Security-Contact" => upstream_ontologist::UpstreamDatum::SecurityContact(
                        extract_str_value(py, value)?,
                    ),
                    "Keywords" => {
                        upstream_ontologist::UpstreamDatum::Keywords(value.extract(py).unwrap())
                    }
                    "Maintainer" => {
                        upstream_ontologist::UpstreamDatum::Maintainer(value.extract(py).unwrap())
                    }
                    "Copyright" => {
                        upstream_ontologist::UpstreamDatum::Copyright(value.extract(py).unwrap())
                    }
                    "Documentation" => upstream_ontologist::UpstreamDatum::Documentation(
                        value.extract(py).unwrap(),
                    ),
                    "Go-Import-Path" => {
                        upstream_ontologist::UpstreamDatum::GoImportPath(value.extract(py).unwrap())
                    }
                    "Download" => {
                        upstream_ontologist::UpstreamDatum::Download(value.extract(py).unwrap())
                    }
                    "Wiki" => upstream_ontologist::UpstreamDatum::Wiki(value.extract(py).unwrap()),
                    "MailingList" => {
                        upstream_ontologist::UpstreamDatum::MailingList(value.extract(py).unwrap())
                    }
                    "SourceForge-Project" => {
                        upstream_ontologist::UpstreamDatum::SourceForgeProject(
                            value.extract(py).unwrap(),
                        )
                    }
                    "Archive" => {
                        upstream_ontologist::UpstreamDatum::Archive(value.extract(py).unwrap())
                    }
                    "Demo" => upstream_ontologist::UpstreamDatum::Demo(value.extract(py).unwrap()),
                    "Pecl-Package" => {
                        upstream_ontologist::UpstreamDatum::PeclPackage(value.extract(py).unwrap())
                    }
                    "Haskell-Package" => upstream_ontologist::UpstreamDatum::HaskellPackage(
                        value.extract(py).unwrap(),
                    ),
                    "Funding" => {
                        upstream_ontologist::UpstreamDatum::Funding(value.extract(py).unwrap())
                    }
                    "Changelog" => {
                        upstream_ontologist::UpstreamDatum::Changelog(value.extract(py).unwrap())
                    }
                    "Debian-ITP" => {
                        upstream_ontologist::UpstreamDatum::DebianITP(value.extract(py).unwrap())
                    }
                    "Screenshots" => {
                        upstream_ontologist::UpstreamDatum::Screenshots(value.extract(py).unwrap())
                    }
                    "Cite-As" => {
                        upstream_ontologist::UpstreamDatum::CiteAs(value.extract(py).unwrap())
                    }
                    "Registry" => {
                        upstream_ontologist::UpstreamDatum::Registry(value.extract(py).unwrap())
                    }
                    "Donation" => {
                        upstream_ontologist::UpstreamDatum::Donation(value.extract(py).unwrap())
                    }
                    "Webservice" => {
                        upstream_ontologist::UpstreamDatum::Webservice(value.extract(py).unwrap())
                    }
                    "FAQ" => upstream_ontologist::UpstreamDatum::FAQ(value.extract(py).unwrap()),
                    _ => {
                        return Err(PyValueError::new_err(format!("Unknown field: {}", field)));
                    }
                },
                origin,
                certainty: certainty.map(|s| Certainty::from_str(&s).unwrap()),
            },
        ))
    }

    #[getter]
    fn field(&self) -> PyResult<String> {
        Ok(self.0.datum.field().to_string())
    }

    #[getter]
    fn value(&self, py: Python) -> PyResult<Py<PyAny>> {
        let value = self
            .0
            .datum
            .into_pyobject(py)
            .unwrap()
            .extract::<(String, Py<PyAny>)>()
            .unwrap()
            .1;
        assert!(!value.bind(py).is_instance_of::<PyTuple>());
        Ok(value)
    }

    #[getter]
    fn origin(&self) -> Option<Origin> {
        self.0.origin.clone()
    }

    #[setter]
    fn set_origin(&mut self, origin: Option<Origin>) {
        self.0.origin = origin;
    }

    #[getter]
    fn certainty(&self) -> Option<String> {
        self.0.certainty.map(|c| c.to_string())
    }

    #[setter]
    pub fn set_certainty(&mut self, certainty: Option<String>) {
        self.0.certainty = certainty.map(|s| Certainty::from_str(&s).unwrap());
    }

    fn __eq__(lhs: &Bound<Self>, rhs: &Bound<Self>) -> PyResult<bool> {
        Ok(lhs.borrow().0 == rhs.borrow().0)
    }

    fn __ne__(lhs: &Bound<Self>, rhs: &Bound<Self>) -> PyResult<bool> {
        Ok(lhs.borrow().0 != rhs.borrow().0)
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!("{}: {}", self.0.datum.field(), self.0.datum))
    }

    fn __repr__(slf: PyRef<Self>) -> PyResult<String> {
        Ok(format!(
            "UpstreamDatum({}, {}, {}, certainty={})",
            slf.0.datum.field(),
            slf.0.datum,
            slf.0
                .origin
                .as_ref()
                .map(|s| format!("Some({})", s))
                .unwrap_or_else(|| "None".to_string()),
            slf.0
                .certainty
                .as_ref()
                .map(|c| format!("Some({})", c))
                .unwrap_or_else(|| "None".to_string()),
        ))
    }
}

/// A collection of upstream project metadata.
///
/// Stores multiple UpstreamDatum objects representing various metadata fields
/// for an upstream project. Provides dict-like access to the metadata.
#[pyclass]
struct UpstreamMetadata(pub(crate) upstream_ontologist::UpstreamMetadata);

#[allow(non_snake_case)]
#[pymethods]
impl UpstreamMetadata {
    fn __getitem__(&self, field: &str) -> PyResult<UpstreamDatum> {
        self.0
            .get(field)
            .map(|datum| UpstreamDatum(datum.clone()))
            .ok_or_else(|| PyKeyError::new_err(format!("No such field: {}", field)))
    }

    fn __delitem__(&mut self, field: &str) -> PyResult<()> {
        self.0.remove(field);
        Ok(())
    }

    fn __contains__(&self, field: &str) -> bool {
        self.0.contains_key(field)
    }

    pub fn items(&self) -> Vec<(String, UpstreamDatum)> {
        self.0
            .iter()
            .map(|datum| {
                (
                    datum.datum.field().to_string(),
                    UpstreamDatum(datum.clone()),
                )
            })
            .collect()
    }

    pub fn values(&self) -> Vec<UpstreamDatum> {
        self.0
            .iter()
            .map(|datum| UpstreamDatum(datum.clone()))
            .collect()
    }

    #[pyo3(signature = (field, default=None))]
    pub fn get(&self, py: Python, field: &str, default: Option<Py<PyAny>>) -> Py<PyAny> {
        let default = default.unwrap_or_else(|| py.None());
        let value = self.0.get(field).map(|datum| {
            UpstreamDatum(datum.clone())
                .into_pyobject(py)
                .unwrap()
                .into()
        });

        value.unwrap_or(default)
    }

    fn __setitem__(&mut self, field: &str, datum: UpstreamDatum) -> PyResult<()> {
        assert_eq!(field, datum.0.datum.field());
        self.0.insert(datum.0);
        Ok(())
    }

    /// Creates a new UpstreamMetadata collection.
    ///
    /// Args:
    ///     **kwargs: Optional keyword arguments of UpstreamDatum objects.
    #[new]
    #[pyo3(signature = (**kwargs))]
    fn new(kwargs: Option<Bound<PyDict>>) -> Self {
        let mut ret = UpstreamMetadata(upstream_ontologist::UpstreamMetadata::new());

        if let Some(kwargs) = kwargs {
            for item in kwargs.items() {
                let datum = item.extract::<UpstreamDatum>().unwrap();
                ret.0.insert(datum.0);
            }
        }

        ret
    }

    /// Creates an UpstreamMetadata collection from a dictionary.
    ///
    /// Args:
    ///     d: Dictionary containing metadata fields and values.
    ///     default_certainty: Default certainty level to apply if not specified.
    ///
    /// Returns:
    ///     A new UpstreamMetadata collection.
    #[classmethod]
    #[pyo3(signature = (d, default_certainty=None))]
    pub fn from_dict(
        _cls: &Bound<PyType>,
        py: Python,
        d: &Bound<PyDict>,
        default_certainty: Option<Certainty>,
    ) -> PyResult<Self> {
        let mut data = Vec::new();
        let di = d.iter();
        for t in di {
            let t: Py<PyAny> = t.into_pyobject(py).unwrap().into();
            let mut datum: upstream_ontologist::UpstreamDatumWithMetadata =
                if let Ok(wm) = t.extract(py) {
                    wm
                } else {
                    let wm: upstream_ontologist::UpstreamDatum = t.extract(py)?;

                    upstream_ontologist::UpstreamDatumWithMetadata {
                        datum: wm,
                        certainty: default_certainty,
                        origin: None,
                    }
                };

            if datum.certainty.is_none() {
                datum.certainty = default_certainty;
            }
            data.push(datum);
        }
        Ok(Self(upstream_ontologist::UpstreamMetadata::from_data(data)))
    }

    pub fn __iter__(slf: PyRef<Self>) -> PyResult<Py<PyAny>> {
        #[pyclass]
        struct UpstreamDatumIter {
            inner: Vec<upstream_ontologist::UpstreamDatumWithMetadata>,
        }
        #[pymethods]
        impl UpstreamDatumIter {
            fn __next__(&mut self) -> Option<UpstreamDatum> {
                self.inner.pop().map(UpstreamDatum)
            }
        }
        Ok(UpstreamDatumIter {
            inner: slf.0.iter().cloned().collect::<Vec<_>>(),
        }
        .into_pyobject(slf.py())
        .unwrap()
        .into())
    }
}

/// Validates and checks upstream metadata for correctness.
///
/// Performs various checks on the metadata to ensure it's valid and consistent.
///
/// Args:
///     metadata: The UpstreamMetadata to check (modified in place).
///     version: Optional version string to validate against.
#[pyfunction]
#[pyo3(signature = (metadata, version=None))]
fn check_upstream_metadata(metadata: &mut UpstreamMetadata, version: Option<&str>) -> PyResult<()> {
    let rt = get_runtime();
    rt.block_on(upstream_ontologist::check_upstream_metadata(
        &mut metadata.0,
        version,
    ));
    Ok(())
}

/// Extends existing upstream metadata by guessing additional fields.
///
/// Analyzes the project at the given path and adds any missing metadata
/// that can be determined with sufficient certainty.
///
/// Args:
///     metadata: The UpstreamMetadata to extend (modified in place).
///     path: Path to the project directory to analyze.
///     minimum_certainty: Minimum certainty level required to add metadata.
///     net_access: Whether to allow network access for gathering metadata.
///     consult_external_directory: Whether to consult external metadata directories.
///
/// Raises:
///     ValueError: If minimum_certainty is invalid.
#[pyfunction]
#[pyo3(signature = (metadata, path, minimum_certainty=None, net_access=None, consult_external_directory=None))]
fn extend_upstream_metadata(
    metadata: &mut UpstreamMetadata,
    path: std::path::PathBuf,
    minimum_certainty: Option<String>,
    net_access: Option<bool>,
    consult_external_directory: Option<bool>,
) -> PyResult<()> {
    let minimum_certainty = minimum_certainty
        .map(|s| s.parse())
        .transpose()
        .map_err(|e: String| PyValueError::new_err(format!("Invalid minimum_certainty: {}", e)))?;
    let rt = get_runtime();
    rt.block_on(upstream_ontologist::extend_upstream_metadata(
        &mut metadata.0,
        path.as_path(),
        minimum_certainty,
        net_access,
        consult_external_directory,
    ))?;
    Ok(())
}

/// Guesses upstream metadata by analyzing a project directory.
///
/// Examines the project structure, files, and content to infer metadata
/// such as name, version, homepage, repository, etc.
///
/// Args:
///     path: Path to the project directory to analyze.
///     trust_package: Whether to trust package metadata files.
///     net_access: Whether to allow network access for gathering metadata.
///     consult_external_directory: Whether to consult external metadata directories.
///     check: Whether to perform validation checks on the gathered metadata.
///
/// Returns:
///     An UpstreamMetadata collection with the guessed metadata.
#[pyfunction]
#[pyo3(signature = (path, trust_package=None, net_access=None, consult_external_directory=None, check=None))]
fn guess_upstream_metadata(
    path: std::path::PathBuf,
    trust_package: Option<bool>,
    net_access: Option<bool>,
    consult_external_directory: Option<bool>,
    check: Option<bool>,
) -> PyResult<UpstreamMetadata> {
    let rt = get_runtime();
    Ok(UpstreamMetadata(rt.block_on(
        upstream_ontologist::guess_upstream_metadata(
            path.as_path(),
            trust_package,
            net_access,
            consult_external_directory,
            check,
        ),
    )?))
}

/// Guesses upstream metadata and returns items as they are discovered.
///
/// Similar to guess_upstream_metadata but returns a list of individual
/// metadata items rather than a collection.
///
/// Args:
///     path: Path to the project directory to analyze.
///     trust_package: Whether to trust package metadata files.
///     minimum_certainty: Minimum certainty level required to include metadata.
///
/// Returns:
///     A list of UpstreamDatum objects.
///
/// Raises:
///     ValueError: If minimum_certainty is invalid.
#[pyfunction]
#[pyo3(signature = (path, trust_package=None, minimum_certainty=None))]
fn guess_upstream_metadata_items(
    py: Python,
    path: std::path::PathBuf,
    trust_package: Option<bool>,
    minimum_certainty: Option<String>,
) -> PyResult<Vec<Py<PyAny>>> {
    let rt = get_runtime();
    let metadata = rt.block_on(
        upstream_ontologist::guess_upstream_metadata_items(
            path.as_path(),
            trust_package,
            minimum_certainty
                .map(|s| s.parse())
                .transpose()
                .map_err(|e: String| {
                    PyValueError::new_err(format!("Invalid minimum_certainty: {}", e))
                })?,
        )
        .collect::<Vec<_>>(),
    );
    Ok(metadata
        .into_iter()
        .filter_map(|datum| datum.ok())
        .map(|datum| datum.into_pyobject(py).unwrap().into())
        .collect::<Vec<Py<PyAny>>>())
}

/// Fixes common issues in upstream metadata.
///
/// Applies corrections to metadata values, such as fixing malformed URLs,
/// normalizing formats, and resolving inconsistencies.
///
/// Args:
///     metadata: The UpstreamMetadata to fix (modified in place).
#[pyfunction]
fn fix_upstream_metadata(metadata: &mut UpstreamMetadata) -> PyResult<()> {
    let rt = get_runtime();
    rt.block_on(upstream_ontologist::fix_upstream_metadata(&mut metadata.0));
    Ok(())
}

/// Updates metadata from an iterator of guessed items.
///
/// Merges guessed metadata items into an existing metadata collection,
/// preferring more certain values and avoiding duplicates.
///
/// Args:
///     metadata: The UpstreamMetadata to update (modified in place).
///     items_iter: An iterator of UpstreamDatum objects to merge.
///
/// Returns:
///     A list of newly added UpstreamDatum objects.
#[pyfunction]
fn update_from_guesses(
    py: Python,
    metadata: &mut UpstreamMetadata,
    items_iter: Py<PyAny>,
) -> PyResult<Vec<UpstreamDatum>> {
    let mut items = vec![];
    loop {
        let item = match items_iter.call_method0(py, "__next__") {
            Ok(item) => item,
            Err(e) => {
                if e.is_instance_of::<PyStopIteration>(py) {
                    break;
                }
                return Err(e);
            }
        };
        items.push(item.extract::<UpstreamDatum>(py)?);
    }
    Ok(upstream_ontologist::update_from_guesses(
        metadata.0.mut_items(),
        items.into_iter().map(|datum| datum.0),
    )
    .into_iter()
    .map(UpstreamDatum)
    .collect())
}

/// Python module for upstream project metadata detection and management.
///
/// This module provides functionality to discover, validate, and manage
/// metadata about upstream software projects, such as names, versions,
/// homepages, repositories, licenses, and more.
#[pymodule]
fn _upstream_ontologist(m: &Bound<PyModule>) -> PyResult<()> {
    pyo3_log::init();
    m.add_wrapped(wrap_pyfunction!(drop_vcs_in_scheme))?;
    m.add_wrapped(wrap_pyfunction!(canonical_git_repo_url))?;
    m.add_wrapped(wrap_pyfunction!(find_public_repo_url))?;
    m.add_wrapped(wrap_pyfunction!(fixup_rcp_style_git_repo_url))?;
    m.add_wrapped(wrap_pyfunction!(check_upstream_metadata))?;
    m.add_wrapped(wrap_pyfunction!(extend_upstream_metadata))?;
    m.add_wrapped(wrap_pyfunction!(guess_upstream_metadata))?;
    m.add_wrapped(wrap_pyfunction!(fix_upstream_metadata))?;
    m.add_wrapped(wrap_pyfunction!(guess_upstream_metadata_items))?;
    m.add_wrapped(wrap_pyfunction!(update_from_guesses))?;
    m.add_wrapped(wrap_pyfunction!(find_secure_repo_url))?;
    m.add_wrapped(wrap_pyfunction!(convert_cvs_list_to_str))?;
    m.add_wrapped(wrap_pyfunction!(fixup_broken_git_details))?;
    m.add_class::<UpstreamMetadata>()?;
    m.add_class::<UpstreamDatum>()?;
    m.add_wrapped(wrap_pyfunction!(known_bad_guess))?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
