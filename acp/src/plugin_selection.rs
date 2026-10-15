use std::sync::Arc;

use a3s_code_core::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilityProjectionError,
    CapabilityRuntimeError, CapabilitySet, CapabilitySource, CapabilityValue,
    CodeCatalogGeneration, ReconciledPackageGeneration, RetainedUseGeneration,
    SessionCapabilityBatch, Sha256Digest, UseCapabilityGeneration, UseGenerationLeaseError,
    UseGenerationLeaseProvider,
};
use a3s_code_core::mcp::McpProjectionAdapter;
use a3s_code_core::AgentSession;
use a3s_use_reconcile::{
    Publication, ReconcileStore, SelectionCursor, SelectionLease, SurfaceKind,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Native launcher-owned selection. ACP metadata cannot select a filesystem root.
#[derive(Clone, Debug)]
pub struct PluginSelectionHost {
    store: ReconcileStore,
    cursor: SelectionCursor,
    generation: UseCapabilityGeneration,
    mcp_names: Vec<String>,
    skill_files: Vec<std::path::PathBuf>,
    workspace: crate::plugin_workspace::WorkspaceBinding,
}

impl PluginSelectionHost {
    pub fn cursor(&self) -> &SelectionCursor {
        &self.cursor
    }
    pub async fn current(
        store: ReconcileStore,
        id: &str,
        workspace: &std::path::Path,
    ) -> Result<Self> {
        let workspace = crate::plugin_workspace::WorkspaceBinding::new(workspace)?;
        let snapshot = store
            .selection(id)
            .await?
            .context("plugin selection is missing")?;
        if !snapshot.is_published() {
            anyhow::bail!("plugin selection is withdrawn");
        }
        let cursor = snapshot.cursor();
        let generation = UseCapabilityGeneration::selection(
            &cursor.selection_id,
            cursor.generation,
            Sha256Digest::new(&cursor.revision)?,
        )?;
        let lease = store.acquire_selection(&cursor).await?;
        let mcp_names = lease
            .publications()
            .flat_map(|publication| &publication.surfaces)
            .filter(|surface| surface.kind == SurfaceKind::Mcp)
            .map(|surface| surface.id.clone())
            .collect();
        let mut skill_files = Vec::new();
        for surface in lease
            .publications()
            .flat_map(|publication| &publication.surfaces)
        {
            if surface.kind != SurfaceKind::Skill {
                continue;
            }
            match canonical_skill_file(&surface.directory) {
                Ok(path) => skill_files.push(path),
                Err(error) => tracing::warn!(
                    skill = %surface.id,
                    %error,
                    "native Skill file is missing; leaving it unadmitted"
                ),
            }
        }
        Ok(Self {
            store,
            cursor,
            generation,
            mcp_names,
            skill_files,
            workspace,
        })
    }

    pub(crate) fn session_options(
        &self,
        options: a3s_code_core::SessionOptions,
    ) -> a3s_code_core::SessionOptions {
        options.with_projected_skill_files(self.skill_files.clone())
    }

    pub(crate) fn validate_workspace(&self, workspace: &std::path::Path) -> Result<()> {
        self.workspace.validate(workspace)
    }

    pub(crate) fn contains_mcp_server(&self, name: &str) -> bool {
        self.mcp_names.iter().any(|selected| selected == name)
    }

    /// Build runtime values from verified managed publications.
    /// A preparing MCP adapter also retains the selected bytes before any Run exists.
    ///
    /// A package that fails structural admission fails this combined batch. Session
    /// admission uses [`Self::apply`], which keeps a refused package out of the catalog.
    pub async fn batch(&self, generation: CodeCatalogGeneration) -> Result<SessionCapabilityBatch> {
        self.workspace.validate_current()?;
        let lease = self.retain_selection().await?;
        let mut admitted = Vec::new();
        for publication in lease._lease.publications() {
            if let Some(package) = self.admit_package(publication).await? {
                admitted.push(package);
            }
        }
        self.materialize(generation, &admitted, &lease)
    }

    /// Publish each admitted package on its own catalog generation.
    ///
    /// The catalog replaces the current projection, so a later package carries the
    /// packages already committed. A failed package rolls back only that attempt.
    /// Catalog compare-and-swap is single-writer, so packages commit in order.
    pub async fn apply(&self, session: &AgentSession) -> Result<()> {
        self.validate_workspace(session.workspace())?;
        self.workspace.validate_current()?;
        let lease = self.retain_selection().await?;
        let mut admitted = Vec::new();
        for publication in lease._lease.publications() {
            let package = match self.admit_package(publication).await {
                Ok(Some(package)) => package,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        package = %publication.package_id,
                        %error,
                        "plugin package was refused"
                    );
                    continue;
                }
            };
            admitted.push(package);
            let Some(generation) = session
                .capability_catalog_stamp()
                .generation()
                .checked_next()
            else {
                anyhow::bail!("Code capability generation is exhausted");
            };
            let batch = match self.materialize(generation, &admitted, &lease) {
                Ok(batch) => batch,
                Err(error) => {
                    admitted.pop();
                    tracing::warn!(
                        package = %publication.package_id,
                        %error,
                        "plugin package was refused"
                    );
                    continue;
                }
            };
            if let Err(error) = session
                .apply_capability_batch(batch, session.session_cancel_token())
                .await
            {
                admitted.pop();
                if session_admission_error(&error) {
                    return Err(error.into());
                }
                tracing::warn!(
                    package = %publication.package_id,
                    %error,
                    "plugin package was not admitted"
                );
            }
        }
        Ok(())
    }

    async fn retain_selection(&self) -> Result<Arc<RetainedSelection>> {
        Ok(Arc::new(RetainedSelection {
            generation: self.generation.clone(),
            _lease: self.store.acquire_selection(&self.cursor).await?,
        }))
    }

    async fn admit_package(&self, publication: &Publication) -> Result<Option<AdmittedPackage>> {
        let cursor = publication.cursor();
        let source = CapabilitySource::use_reconcile(
            self.generation.clone(),
            ReconciledPackageGeneration::new(
                &cursor.package_id,
                cursor.generation,
                Sha256Digest::new(&cursor.revision)?,
            )?,
        )?;
        let mut descriptors = Vec::new();
        let mut values = Vec::new();
        let mut servers = Vec::new();
        for surface in &publication.surfaces {
            match surface.kind {
                SurfaceKind::Tool => {
                    let entry = surface
                        .entry
                        .clone()
                        .context("native Tool entry is missing")?;
                    let descriptor = CapabilityDescriptor::new(
                        &source,
                        CapabilityKind::Tool,
                        &surface.id,
                        &surface.id,
                        Sha256Digest::new(&surface.content_sha256)?,
                        Vec::new(),
                    )?;
                    values.push((
                        descriptor.id().clone(),
                        CapabilityValue::Tool(crate::plugin_tools::from_publication(
                            &surface.id,
                            entry,
                            self.workspace.clone(),
                        )?),
                    ));
                    descriptors.push(descriptor);
                }
                SurfaceKind::Skill => {
                    let Some(path) = canonical_skill_file(&surface.directory).ok() else {
                        tracing::warn!(
                            skill = %surface.id,
                            "native Skill file is missing; leaving it unadmitted"
                        );
                        continue;
                    };
                    let parsed = tokio::task::spawn_blocking(move || {
                        a3s_code_core::skills::Skill::from_file(path)
                    })
                    .await?;
                    let skill = match parsed {
                        Ok(skill) => skill,
                        Err(error) => {
                            tracing::warn!(
                                skill = %surface.id,
                                %error,
                                "native Skill file is unreadable; leaving it unadmitted"
                            );
                            continue;
                        }
                    };
                    let descriptor = CapabilityDescriptor::new(
                        &source,
                        CapabilityKind::Skill,
                        &surface.id,
                        &skill.name,
                        Sha256Digest::new(&surface.content_sha256)?,
                        Vec::new(),
                    )?;
                    values.push((
                        descriptor.id().clone(),
                        CapabilityValue::Skill(Arc::new(skill)),
                    ));
                    descriptors.push(descriptor);
                }
                SurfaceKind::Mcp => match super::plugin_mcp::admit(surface)? {
                    super::plugin_mcp::McpAdmission::ExternalRuntime => {
                        tracing::warn!(
                            server = %surface.id,
                            "native MCP has no managed executable; leaving it unadmitted"
                        );
                    }
                    super::plugin_mcp::McpAdmission::Ready(config) => {
                        let descriptor = CapabilityDescriptor::new(
                            &source,
                            CapabilityKind::Mcp,
                            &surface.id,
                            &surface.id,
                            Sha256Digest::new(&surface.content_sha256)?,
                            Vec::new(),
                        )?;
                        servers.push((descriptor.id().clone(), *config));
                        descriptors.push(descriptor);
                    }
                },
                _ => {}
            }
        }
        if descriptors.is_empty() {
            return Ok(None);
        }
        Ok(Some(AdmittedPackage {
            source,
            descriptors,
            values,
            servers,
        }))
    }

    fn materialize(
        &self,
        generation: CodeCatalogGeneration,
        packages: &[AdmittedPackage],
        lease: &Arc<RetainedSelection>,
    ) -> Result<SessionCapabilityBatch> {
        let contributions = packages
            .iter()
            .map(|package| {
                CapabilityContribution::new(package.source.clone(), package.descriptors.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let set =
            CapabilitySet::from_use_projection(generation, self.generation.clone(), contributions)?;
        let mut batch = SessionCapabilityBatch::from_use_projection(set, Arc::new(self.clone()))?;
        for package in packages {
            for (id, value) in &package.values {
                batch.stage_value(id.clone(), value.clone())?;
            }
            for (id, config) in &package.servers {
                batch.stage(
                    id.clone(),
                    McpProjectionAdapter::new(config.clone())
                        .with_working_directory(self.workspace.root().to_owned())
                        .with_retained_use_generation(lease.clone()),
                )?;
            }
        }
        Ok(batch)
    }
}

struct AdmittedPackage {
    source: CapabilitySource,
    descriptors: Vec<CapabilityDescriptor>,
    values: Vec<(a3s_code_core::capability::CapabilityId, CapabilityValue)>,
    servers: Vec<(
        a3s_code_core::capability::CapabilityId,
        a3s_code_core::mcp::McpServerConfig,
    )>,
}

fn canonical_skill_file(directory: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::canonicalize(directory.join("SKILL.md"))
}

/// A package that cannot connect is omitted. Workspace, cancellation, and catalog
/// integrity failures still refuse the session.
fn session_admission_error(error: &CapabilityRuntimeError) -> bool {
    !matches!(
        error,
        CapabilityRuntimeError::Projection(CapabilityProjectionError::PrepareFailed { .. })
    )
}

struct RetainedSelection {
    generation: UseCapabilityGeneration,
    _lease: SelectionLease,
}

impl RetainedUseGeneration for RetainedSelection {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
}

#[async_trait]
impl UseGenerationLeaseProvider for PluginSelectionHost {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }

    fn validate_workspace(
        &self,
        workspace: &std::path::Path,
    ) -> Result<(), UseGenerationLeaseError> {
        self.workspace
            .validate(workspace)
            .map_err(|error| UseGenerationLeaseError::new(error.to_string()))
    }

    async fn acquire(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError> {
        if cancellation.is_cancelled() {
            return Err(UseGenerationLeaseError::new(
                "plugin selection admission cancelled",
            ));
        }
        self.workspace
            .validate_current()
            .map_err(|error| UseGenerationLeaseError::new(error.to_string()))?;
        // Await acquisition even on cancellation so a blocking file-verification task
        // cannot leave an unobserved retained lease running after admission returns.
        let lease = self
            .store
            .acquire_selection(&self.cursor)
            .await
            .map_err(|error| UseGenerationLeaseError::new(error.to_string()))?;
        if cancellation.is_cancelled() {
            return Err(UseGenerationLeaseError::new(
                "plugin selection admission cancelled",
            ));
        }
        self.workspace
            .validate_current()
            .map_err(|error| UseGenerationLeaseError::new(error.to_string()))?;
        Ok(Box::new(RetainedSelection {
            generation: self.generation.clone(),
            _lease: lease,
        }))
    }
}

#[cfg(test)]
#[path = "plugin_selection_tests.rs"]
mod tests;
