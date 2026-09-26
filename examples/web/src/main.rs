//! The demo's entry point: options from the command line natively, and from
//! the page's query string in a browser, so `?scene=city-alley&night=1` opens
//! the demo on one view.
use ashlar_web::{LodView, Options};

/// Fold one `key`/`value` pair into the options. Both sources speak the same
/// keys, so a native flag and a query parameter are one vocabulary.
fn set(options: &mut Options, key: &str, value: &str) -> Result<(), String> {
    let number = |value: &str| {
        value
            .parse::<i32>()
            .map_err(|_| format!("{key} wants a whole number, not {value:?}"))
    };
    match key {
        "scene" => {
            if !ashlar_web::catalog::SCENES
                .iter()
                .any(|scene| scene.name == value)
            {
                return Err(format!(
                    "unknown scene {value:?}; see --help for scene names"
                ));
            }
            options.scene = Some(value.to_owned());
        }
        "night" => options.night = Some(value != "0" && value != "false"),
        "day" => options.night = Some(value == "0" || value == "false"),
        "material" => options.material = Some(value.to_owned()),
        "param" => {
            let (name, value) = value
                .split_once('=')
                .ok_or_else(|| format!("param wants name=value, not {value:?}"))?;
            options.params.push((name.to_owned(), value.to_owned()));
        }
        "storey" => options.max_storey = Some(number(value)?),
        "interior" => options.hide_exterior = value != "0" && value != "false",
        "lod" => {
            options.lod = match value {
                "bands" => LodView::Bands,
                "tint" => LodView::Tint,
                level => LodView::Force(
                    usize::try_from(number(level)?)
                        .map_err(|_| format!("lod wants bands, tint or a level, not {level:?}"))?,
                ),
            }
        }
        "walk" => options.walk = value != "0" && value != "false",
        "assets" => options.assets = Some(value.to_owned()),
        "screenshot" => options.screenshot = Some(value.to_owned()),
        _ => return Err(format!("unknown option {key:?}")),
    }
    Ok(())
}

/// `--key value` pairs, and `--night`, `--day`, `--interior` and `--walk`
/// alone.
#[cfg(not(target_arch = "wasm32"))]
fn options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let key = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("expected --option, found {flag:?}"))?;
        let value = if matches!(key, "night" | "day" | "interior" | "walk") {
            "1".to_owned()
        } else {
            args.next()
                .ok_or_else(|| format!("--{key} wants a value"))?
        };
        set(&mut options, key, &value)?;
    }
    Ok(options)
}

/// The query string, `?scene=metropolis&night=0`. A key the demo does not
/// know is ignored rather than refused: a link somebody pasted with a stray
/// tracking parameter should still open.
#[cfg(target_arch = "wasm32")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the native reader's signature, which can refuse a flag"
)]
fn options() -> Result<Options, String> {
    let search = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default();
    Ok(query_options(&search))
}

#[cfg(any(target_arch = "wasm32", test))]
fn query_options(search: &str) -> Options {
    let mut options = Options::default();
    for (key, value) in form_urlencoded::parse(search.trim_start_matches('?').as_bytes()) {
        if key == "assets" || key == "screenshot" {
            continue;
        }
        if let Err(error) = set(&mut options, &key, &value) {
            tracing::warn!("{error}");
        }
    }
    options
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    if matches!(std::env::args().nth(1).as_deref(), Some("--help" | "-h")) {
        println!(
            "ashlar-web — explorable buildings and materials

Usage: ashlar-web [OPTIONS]

  --scene NAME        Open a scene (listed below)
  --night, --day       Set lighting
  --material NAME     Open the material gallery
  --param NAME=VALUE   Override a parameter; repeat for multiple parameters
  --storey NUMBER     Hide higher storeys
  --interior          Hide exterior geometry
  --lod bands|tint|N   Show distance bands, tint levels, or force a level
  --walk              Start at street level
  --assets PATH       Asset root (default: target/web/assets)
  --screenshot PATH   Capture a loaded frame and exit
  -h, --help          Show this help

Bake the assets first with `just site`.

Scenes:"
        );
        for scene in ashlar_web::catalog::SCENES {
            println!("  {}", scene.name);
        }
        return;
    }
    match options() {
        Ok(options) => {
            ashlar_web::app(options).run();
        }
        Err(error) => {
            eprintln!("ashlar-web: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_decodes_colours_and_repeated_overrides_and_ignores_native_paths() {
        let options = query_options(
            "?scene=city%2Dalley&param=color%3D0.2%2C0.3%2C0.4&param=wear%3D0.8&night&assets=%2Ftmp&screenshot=out.png",
        );
        assert_eq!(options.scene.as_deref(), Some("city-alley"));
        assert_eq!(
            options.params,
            [
                ("color".into(), "0.2,0.3,0.4".into()),
                ("wear".into(), "0.8".into())
            ]
        );
        assert_eq!(options.night, Some(true));
        assert!(options.assets.is_none());
        assert!(options.screenshot.is_none());
    }

    #[test]
    fn an_unknown_scene_is_an_error_instead_of_a_silent_default() {
        assert!(set(&mut Options::default(), "scene", "city-ally").is_err());
    }
}
