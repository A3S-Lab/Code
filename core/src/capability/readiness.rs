use std::collections::{BTreeMap, BTreeSet};

use super::{
    CapabilityId, CapabilityProjectionError, CapabilitySet, CodeCatalogGeneration, Sha256Digest,
    MAX_CAPABILITIES, MAX_CAPABILITY_DEPENDENCY_EDGES,
};

pub const CAPABILITY_READINESS_PLAN_SCHEMA: &str = "a3s.code.capability-readiness-plan.v1";
pub const MAX_CAPABILITY_READINESS_WAVES: usize = MAX_CAPABILITIES;

/// Deterministic dependency-first ordering for one immutable capability set.
///
/// The plan reads only descriptor-level surface edges already published in a
/// [`CapabilitySet`]. It does not inspect package manifests, select versions,
/// resolve A3S Use dependencies, or provide a service container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityReadinessPlan {
    generation: CodeCatalogGeneration,
    digest: Sha256Digest,
    waves: Vec<Vec<CapabilityId>>,
    activation_order: Vec<CapabilityId>,
    edge_count: usize,
    max_wave_width: usize,
}

impl CapabilityReadinessPlan {
    /// Build the bounded, minimal readiness waves for one exact catalog.
    ///
    /// Kahn's algorithm is iterative so the configured maximum-depth chain
    /// cannot consume the call stack. `BTreeMap` and `BTreeSet` make both wave
    /// membership and the flattened activation order platform-independent.
    pub fn from_set(set: &CapabilitySet) -> Result<Self, CapabilityProjectionError> {
        if set.len() > MAX_CAPABILITIES {
            return Err(CapabilityProjectionError::ReadinessBoundExceeded {
                field: "capabilities",
                max: MAX_CAPABILITIES,
            });
        }

        let mut remaining_dependencies = BTreeMap::<CapabilityId, usize>::new();
        let mut dependents = BTreeMap::<CapabilityId, Vec<CapabilityId>>::new();
        for (id, descriptor) in set.iter() {
            remaining_dependencies.insert(id.clone(), descriptor.dependencies().len());
            dependents.insert(id.clone(), Vec::new());
        }

        let mut edge_count = 0_usize;
        for (id, descriptor) in set.iter() {
            for dependency in descriptor.dependencies() {
                edge_count = edge_count.checked_add(1).ok_or(
                    CapabilityProjectionError::ReadinessBoundExceeded {
                        field: "dependency_edges",
                        max: MAX_CAPABILITY_DEPENDENCY_EDGES,
                    },
                )?;
                if edge_count > MAX_CAPABILITY_DEPENDENCY_EDGES {
                    return Err(CapabilityProjectionError::ReadinessBoundExceeded {
                        field: "dependency_edges",
                        max: MAX_CAPABILITY_DEPENDENCY_EDGES,
                    });
                }
                let Some(consumers) = dependents.get_mut(dependency) else {
                    return Err(CapabilityProjectionError::ReadinessDependencyMissing {
                        capability: id.to_string(),
                        dependency: dependency.to_string(),
                    });
                };
                consumers.push(id.clone());
            }
        }
        for consumers in dependents.values_mut() {
            consumers.sort();
        }

        let mut ready = remaining_dependencies
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
            .collect::<BTreeSet<_>>();
        let mut waves = Vec::new();
        let mut activation_order = Vec::with_capacity(set.len());
        let mut max_wave_width = 0_usize;

        while !ready.is_empty() {
            if waves.len() >= MAX_CAPABILITY_READINESS_WAVES {
                return Err(CapabilityProjectionError::ReadinessBoundExceeded {
                    field: "readiness_waves",
                    max: MAX_CAPABILITY_READINESS_WAVES,
                });
            }
            let wave = std::mem::take(&mut ready).into_iter().collect::<Vec<_>>();
            max_wave_width = max_wave_width.max(wave.len());
            let mut next = BTreeSet::new();

            for id in &wave {
                activation_order.push(id.clone());
                let Some(consumers) = dependents.get(id) else {
                    return Err(CapabilityProjectionError::ReadinessGraphInvariant {
                        message: "a planned capability has no dependent index entry",
                    });
                };
                for consumer in consumers {
                    let Some(count) = remaining_dependencies.get_mut(consumer) else {
                        return Err(CapabilityProjectionError::ReadinessGraphInvariant {
                            message: "a dependent capability has no readiness counter",
                        });
                    };
                    let Some(updated) = count.checked_sub(1) else {
                        return Err(CapabilityProjectionError::ReadinessGraphInvariant {
                            message: "a dependency edge was released more than once",
                        });
                    };
                    *count = updated;
                    if updated == 0 {
                        next.insert(consumer.clone());
                    }
                }
            }

            waves.push(wave);
            ready = next;
        }

        if activation_order.len() != set.len() {
            let blocked_count = set.len() - activation_order.len();
            let Some(first_blocked) = remaining_dependencies
                .iter()
                .find_map(|(id, count)| (*count > 0).then_some(id.to_string()))
            else {
                return Err(CapabilityProjectionError::ReadinessGraphInvariant {
                    message: "readiness traversal stopped with unaccounted capabilities",
                });
            };
            return Err(CapabilityProjectionError::DependencyCycle {
                first_blocked,
                blocked_count,
            });
        }

        Ok(Self {
            generation: set.generation(),
            digest: set.digest().clone(),
            waves,
            activation_order,
            edge_count,
            max_wave_width,
        })
    }

    pub const fn schema(&self) -> &'static str {
        CAPABILITY_READINESS_PLAN_SCHEMA
    }

    pub const fn generation(&self) -> CodeCatalogGeneration {
        self.generation
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub fn capability_count(&self) -> usize {
        self.activation_order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.activation_order.is_empty()
    }

    pub const fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub fn depth(&self) -> usize {
        self.waves.len()
    }

    pub const fn max_wave_width(&self) -> usize {
        self.max_wave_width
    }

    pub fn waves(&self) -> &[Vec<CapabilityId>] {
        &self.waves
    }

    pub fn activation_order(&self) -> &[CapabilityId] {
        &self.activation_order
    }

    pub(super) fn matches(&self, set: &CapabilitySet) -> bool {
        self.generation == set.generation() && self.digest == *set.digest()
    }
}
