use std::cell::RefCell;

use actix_web::http::header::ContentType;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use minijinja::value::{Rest, Value};
use minijinja::{context, path_loader, Environment, Error, ErrorKind};

thread_local! {
    static CURRENT_REQUEST: RefCell<Option<HttpRequest>> = RefCell::default()
}

/// Binds the given request to a thread local for `url_for`.
fn with_bound_req<F, R>(req: &HttpRequest, f: F) -> R
where
    F: FnOnce() -> R,
{
    CURRENT_REQUEST.with(|current_req| *current_req.borrow_mut() = Some(req.clone()));
    let rv = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    CURRENT_REQUEST.with(|current_req| current_req.borrow_mut().take());
    match rv {
        Ok(rv) => rv,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

struct AppState {
    env: minijinja::Environment<'static>,
}

impl AppState {
    /// Helper function to render a template to an HTTP response with a bound request.
    pub fn render_template(&self, name: &str, req: &HttpRequest, ctx: Value) -> HttpResponse {
        with_bound_req(req, || {
            let tmpl = self.env.get_template(name).unwrap();
            let rv = tmpl.render(ctx).unwrap();
            HttpResponse::Ok()
                .content_type(ContentType::html())
                .body(rv)
        })
    }
}

/// Helper function that is added to templates to invoke `url_for` on the bound request.
fn url_for(name: &str, args: Rest<String>) -> Result<Value, Error> {
    CURRENT_REQUEST.with(|current_req| {
        Ok(current_req
            .borrow()
            .as_ref()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    "url_for requires an http request",
                )
            })?
            .url_for(name, &args[..])
            .map_err(|err| {
                Error::new(ErrorKind::InvalidOperation, "failed to generate url").with_source(err)
            })?
            .to_string()
            .into())
    })
}

async fn index(app_state: web::Data<AppState>, req: HttpRequest) -> impl Responder {
    app_state.render_template("index.html", &req, context! { name => "World" })
}

async fn user(
    app_state: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<(u64,)>,
) -> impl Responder {
    app_state.render_template("user.html", &req, context! { user_id => path.0 })
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));
    env.add_function("url_for", url_for);

    let state = web::Data::new(AppState { env });

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(web::resource("/").name("index").route(web::get().to(index)))
            .service(
                web::resource("/user/{user_id}")
                    .name("user")
                    .route(web::get().to(user)),
            )
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
