use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use minijinja::context;
use minijinja::{path_loader, Environment};
use minijinja_autoreload::AutoReloader;

fn main() {
    // If DISABLE_AUTORELOAD is set, then the path tracking is disabled.
    let disable_autoreload = env::var("DISABLE_AUTORELOAD").as_deref() == Ok("1");

    // If FAST_AUTORELOAD is set, then fast reloading is enabled.
    let fast_autoreload = env::var("FAST_AUTORELOAD").as_deref() == Ok("1");

    // The closure is invoked every time the environment is outdated to
    // recreate it.
    let reloader = AutoReloader::new(move |notifier| {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut env = Environment::new();
        env.set_loader(path_loader(&template_path));

        if fast_autoreload {
            notifier.set_fast_reload(true);
        }

        // if watch_path is never called, no fs watcher is created
        if !disable_autoreload {
            notifier.watch_path(&template_path, true);
        }
        Ok(env)
    });

    // keep running the template.  to experiment change the template.txt file or
    // rename or change the include file.
    for iteration in 1.. {
        // acquire gets the latest version of the environment.
        let env = reloader.acquire_env().unwrap();
        let tmpl = env.get_template("template.txt").unwrap();
        println!("{}", tmpl.render(context!(iteration)).unwrap());
        thread::sleep(Duration::from_secs(1));
    }
}
