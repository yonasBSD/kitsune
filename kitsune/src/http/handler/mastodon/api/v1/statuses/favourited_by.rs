use crate::{
    consts::default_limit,
    http::{
        extractor::MastodonAuthExtractor,
        pagination::{LinkHeader, PaginatedJsonResponse},
    },
};
use axum::{
    Json, debug_handler,
    extract::{OriginalUri, Path, State},
};
use axum_extra::extract::Query;
use futures_util::{TryFutureExt, TryStreamExt};
use kitsune_error::{Error, Result};
use kitsune_mastodon::MastodonMapper;
use kitsune_service::post::{GetAccountsInteractingWithPost, PostService};
use kitsune_type::mastodon::Account;
use kitsune_url::UrlService;
use serde::Deserialize;
use speedy_uuid::Uuid;

#[derive(Deserialize)]
pub struct GetQuery {
    min_id: Option<Uuid>,
    max_id: Option<Uuid>,
    since_id: Option<Uuid>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[debug_handler(state = crate::state::Zustand)]
pub async fn get(
    State(post_service): State<PostService>,
    State(mastodon_mapper): State<MastodonMapper>,
    State(url_service): State<UrlService>,
    OriginalUri(original_uri): OriginalUri,
    Query(query): Query<GetQuery>,
    user_data: Option<MastodonAuthExtractor>,
    Path(id): Path<Uuid>,
) -> Result<PaginatedJsonResponse<Account>> {
    let fetching_account_id = user_data.map(|user_data| user_data.0.account.id);
    let get_favourites = GetAccountsInteractingWithPost::builder()
        .post_id(id)
        .fetching_account_id(fetching_account_id)
        .limit(query.limit)
        .since_id(query.since_id)
        .min_id(query.min_id)
        .max_id(query.max_id)
        .build();

    let accounts: Vec<Account> = post_service
        .favourited_by(get_favourites)
        .await?
        .map_err(Error::from)
        .and_then(|acc| mastodon_mapper.map(acc).map_err(Error::from))
        .try_collect()
        .await?;

    let link_header = LinkHeader::new(
        &accounts,
        query.limit,
        &url_service.base_url(),
        original_uri.path(),
        |a| a.id,
    );

    Ok((link_header, Json(accounts)))
}
