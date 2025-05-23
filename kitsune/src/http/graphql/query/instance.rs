use crate::http::graphql::{ContextExt, types::Instance};
use async_graphql::{Context, Object, Result};
use kitsune_core::consts::VERSION;

#[derive(Default)]
pub struct InstanceQuery;

#[Object]
impl InstanceQuery {
    #[allow(clippy::unused_async)]
    pub async fn instance(&self, ctx: &Context<'_>) -> Result<Instance> {
        let state = ctx.state();
        let instance_service = &state.service.instance;
        let url_service = &state.service.url;
        let captcha = state.service.captcha.backend.clone().map(Into::into);

        let character_limit = instance_service.character_limit();
        let description = instance_service.description().into();
        let domain = url_service.webfinger_domain().into();
        let local_post_count = instance_service.local_post_count().await?;
        let name = instance_service.name().into();
        let registrations_open = instance_service.registrations_open();
        let user_count = instance_service.user_count().await?;

        Ok(Instance {
            captcha,
            character_limit,
            description,
            domain,
            local_post_count,
            name,
            registrations_open,
            user_count,
            version: VERSION,
        })
    }
}
