use crate::oauth2::{OAuthEndpoint, OAuthOwnerSolicitor};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    debug_handler,
    extract::{OriginalUri, State},
    response::{Html, Redirect},
    Form,
};
use axum_extra::{
    either::{Either, Either3},
    extract::{
        cookie::{Cookie, Expiration, SameSite},
        SignedCookieJar,
    },
};
use axum_flash::{Flash, IncomingFlashes};
use cursiv::CsrfHandle;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use kitsune_db::{model::user::User, schema::users, with_connection, PgPool};
use kitsune_error::{Error, Result};
use oxide_auth_async::endpoint::authorization::AuthorizationFlow;
use oxide_auth_axum::{OAuthRequest, OAuthResponse};
use serde::Deserialize;
use speedy_uuid::Uuid;

const UNCONFIRMED_EMAIL_ADDRESS: &str = "Email address is unconfirmed. Check your inbox!";
const WRONG_EMAIL_OR_PASSWORD: &str = "Entered wrong email or password";

#[cfg(feature = "oidc")]
use {
    axum::extract::Query,
    kitsune_db::{model::oauth2, schema::oauth2_applications},
    kitsune_oidc::OidcService,
};

#[cfg(feature = "oidc")]
#[derive(Deserialize)]
pub struct AuthorizeQuery {
    client_id: Uuid,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn get(
    #[cfg(feature = "oidc")] (State(oidc_service), Query(query)): (
        State<Option<OidcService>>,
        Query<AuthorizeQuery>,
    ),

    State(db_pool): State<PgPool>,
    State(oauth_endpoint): State<OAuthEndpoint>,
    cookies: SignedCookieJar,
    csrf_handle: CsrfHandle,
    flash_messages: IncomingFlashes,
    oauth_req: OAuthRequest,
) -> Result<Either3<OAuthResponse, Html<String>, Redirect>> {
    #[cfg(feature = "oidc")]
    if let Some(oidc_service) = oidc_service {
        let application = with_connection!(db_pool, |db_conn| {
            oauth2_applications::table
                .find(query.client_id)
                .filter(oauth2_applications::redirect_uri.eq(query.redirect_uri))
                .get_result::<oauth2::Application>(db_conn)
                .await
        })?;

        let auth_url = oidc_service
            .authorisation_url(application.id, query.scope, query.state)
            .await?;

        return Ok(Either3::E3(Redirect::to(auth_url.as_str())));
    }

    let authenticated_user = if let Some(user_id) = cookies.get("user_id") {
        let id = user_id.value().parse::<Uuid>()?;
        with_connection!(db_pool, |db_conn| {
            users::table.find(id).get_result(db_conn).await
        })?
    } else {
        let messages: Vec<(axum_flash::Level, &str)> = flash_messages.into_iter().collect();
        let page = crate::template::render(
            "oauth/login.html",
            minijinja::context! {
                flash_messages => messages,
            },
        )
        .unwrap();

        return Ok(Either3::E2(Html(page)));
    };

    let solicitor = OAuthOwnerSolicitor::builder()
        .authenticated_user(authenticated_user)
        .csrf_handle(csrf_handle)
        .db_pool(db_pool)
        .build();

    let mut flow = AuthorizationFlow::prepare(oauth_endpoint.with_solicitor(solicitor))?;
    AuthorizationFlow::execute(&mut flow, oauth_req)
        .await
        .map(Either3::E1)
        .map_err(Error::from)
}

#[debug_handler(state = crate::state::Zustand)]
pub async fn post(
    State(db_pool): State<PgPool>,
    OriginalUri(original_url): OriginalUri,
    cookies: SignedCookieJar,
    flash: Flash,
    Form(form): Form<LoginForm>,
) -> Result<Either<(SignedCookieJar, Redirect), (Flash, Redirect)>> {
    let redirect_to = if let Some(path_and_query) = original_url.path_and_query() {
        path_and_query.as_str()
    } else {
        original_url.path()
    };

    let user = with_connection!(db_pool, |db_conn| {
        users::table
            .filter(users::username.eq(form.username))
            .first::<User>(db_conn)
            .await
            .optional()
    })?;

    let Some(user) = user else {
        return Ok(Either::E2((
            flash.error(WRONG_EMAIL_OR_PASSWORD),
            Redirect::to(redirect_to),
        )));
    };

    if user.confirmed_at.is_none() {
        return Ok(Either::E2((
            flash.error(UNCONFIRMED_EMAIL_ADDRESS),
            Redirect::to(redirect_to),
        )));
    }

    let is_valid = blowocking::crypto(move || {
        let password_hash = PasswordHash::new(user.password.as_ref().unwrap())?;
        let argon2 = Argon2::default();

        Ok::<_, Error>(
            argon2
                .verify_password(form.password.as_bytes(), &password_hash)
                .is_ok(),
        )
    })
    .await??;

    if !is_valid {
        return Ok(Either::E2((
            flash.error(WRONG_EMAIL_OR_PASSWORD),
            Redirect::to(redirect_to),
        )));
    }

    // TODO: Bad because no expiration. Either encode an expiration into the cookie and make this basically a shitty JWT
    // or store session IDs instead that are stored in a TTL data structure (these would need to be either storable in-memory or in Redis; similar to OIDC data)
    let user_id_cookie = Cookie::build(("user_id", user.id.to_string()))
        .same_site(SameSite::Strict)
        .expires(Expiration::Session)
        .secure(true);

    Ok(Either::E1((
        cookies.add(user_id_cookie),
        Redirect::to(redirect_to),
    )))
}
