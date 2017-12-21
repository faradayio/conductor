//! Plugin which updates the `labels` in a `dc::File`.

use compose_yml::v2 as dc;
use std::marker::PhantomData;

use errors::*;
use plugins;
use plugins::{Operation, PluginNew, PluginTransform};
use project::Project;

/// Updates the `labels` in a `dc::File`.
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub struct Plugin {
    /// Placeholder field for future hidden fields, to keep this from being
    /// directly constructable.
    _placeholder: PhantomData<()>,
}

impl plugins::Plugin for Plugin {
    fn name(&self) -> &'static str {
        Self::plugin_name()
    }
}

impl PluginNew for Plugin {
    fn plugin_name() -> &'static str {
        "labels"
    }

    fn new(_project: &Project) -> Result<Self> {
        Ok(Plugin {
            _placeholder: PhantomData,
        })
    }
}

impl PluginTransform for Plugin {
    fn transform(
        &self,
        _op: Operation,
        ctx: &plugins::Context,
        file: &mut dc::File,
    ) -> Result<()> {
        for service in &mut file.services.values_mut() {
            // These are intended for easy use as `docker ps --filter`
            // arguments.
            let target = ctx.project.current_target().name();
            service
                .labels
                .insert("io.fdy.cage.target".into(), Some(dc::value(target.into())));
            service
                .labels
                .insert("io.fdy.cage.pod".into(), Some(dc::value(ctx.pod.name().into())));

            // TODO LOW: Remove metadata-only `io.fdy.cage.` labels?
        }
        Ok(())
    }
}
