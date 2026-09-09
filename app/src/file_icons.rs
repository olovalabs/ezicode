use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

const FILE_STEMS: &[(&str, &str)] = &[
    ("Containerfile", "docker"),
    ("Dockerfile", "docker"),
    ("docker-compose.yml", "docker"),
    ("docker-compose.yaml", "docker"),
    ("compose.yml", "docker"),
    ("compose.yaml", "docker"),
    ("Cargo.toml", "cargo"),
    ("Cargo.lock", "cargo"),
    ("package.json", "npm"),
    ("package-lock.json", "npm"),
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
    ("bunfig.toml", "bun"),
    ("yarn.lock", "yarn"),
    ("pnpm-lock.yaml", "pnpm"),
    ("Podfile", "ruby"),
    ("Procfile", "procfile"),
    ("turbo.json", "turbo"),
    ("tsconfig.json", "typescript"),
    ("jsconfig.json", "jsconfig"),
    ("vite.config.js", "vite"),
    ("vite.config.ts", "vite"),
    ("vite.config.mjs", "vite"),
    ("tailwind.config.js", "tailwind"),
    ("tailwind.config.ts", "tailwind"),
    ("next.config.js", "next"),
    ("next.config.mjs", "next"),
    ("next.config.ts", "next"),
    ("astro.config.mjs", "astro"),
    ("astro.config.ts", "astro"),
    (".gitignore", "git"),
    (".gitattributes", "git"),
    (".gitmodules", "git"),
    (".dockerignore", "docker"),
    (".npmrc", "npm"),
    ("pnpm-workspace.yaml", "pnpm"),
    (".prettierignore", "prettier"),
    (".editorconfig", "editorconfig"),
    (".prettierrc", "prettier"),
    (".eslintrc", "eslint"),
    ("LICENSE", "license"),
    ("LICENCE", "license"),
    ("README.md", "markdown"),
    ("CHANGELOG.md", "markdown"),
];

#[rustfmt::skip]
const FILE_SUFFIXES: &[(&str, &str)] = &[
    ("astro", "astro"),
    ("aac", "audio"), ("flac", "audio"), ("m4a", "audio"), ("mka", "audio"), ("mp3", "audio"), ("ogg", "audio"), ("opus", "audio"), ("wav", "audio"), ("wma", "audio"),
    ("bat", "bat"), ("cmd", "bat"),
    ("bun", "bun"), ("lockb", "bun"),
    ("c", "c"), ("h", "c"),
    ("cpp", "cpp"), ("h++", "cpp"), ("cc", "cpp"), ("cppm", "cpp"), ("cxx", "cpp"), ("hh", "cpp"), ("hpp", "cpp"), ("hxx", "cpp"), ("inl", "cpp"), ("ixx", "cpp"),
    ("cs", "csharp"), ("csx", "csharp"),
    ("css", "css"), ("pcss", "postcss"), ("postcss", "postcss"),
    ("dart", "dart"),
    ("d.ts", "typescriptdef"), ("d.mts", "typescriptdef"), ("d.cts", "typescriptdef"),
    ("docker", "docker"),
    ("editorconfig", "editorconfig"),
    ("eex", "elixir"), ("ex", "elixir"), ("exs", "elixir"), ("heex", "elixir"), ("leex", "elixir"),
    ("erl", "erlang"), ("hrl", "erlang"),
    ("eslint.config.cjs", "eslint"), ("eslint.config.cts", "eslint"), ("eslint.config.js", "eslint"), ("eslint.config.mjs", "eslint"), ("eslint.config.mts", "eslint"), ("eslint.config.ts", "eslint"), ("eslintrc", "eslint"), ("eslintrc.js", "eslint"), ("eslintrc.json", "eslint"),
    ("otf", "font"), ("ttf", "font"), ("woff", "font"), ("woff2", "font"),
    ("fs", "fsharp"), ("fsi", "fsharp"), ("fsx", "fsharp"),
    ("git", "git"), ("gitignore", "git"), ("gitattributes", "git"), ("gitmodules", "git"),
    ("go", "go"), ("mod", "go"), ("work", "go"),
    ("gql", "graphql"), ("graphql", "graphql"), ("graphqls", "graphql"),
    ("hs", "haskell"), ("lhs", "haskell"),
    ("htm", "html"), ("html", "html"),
    ("avif", "image"), ("bmp", "image"), ("gif", "image"), ("heic", "image"), ("ico", "image"), ("jpeg", "image"), ("jpg", "image"), ("png", "image"), ("psd", "image"), ("svg", "svg"), ("tiff", "image"), ("webp", "image"),
    ("ini", "ini"), ("conf", "ini"), ("cfg", "ini"),
    ("java", "java"), ("jar", "java"), ("class", "java"),
    ("cjs", "javascript"), ("js", "javascript"), ("mjs", "javascript"),
    ("json", "json"), ("jsonc", "json"), ("json5", "json5"), ("jsonld", "jsonld"),
    ("jl", "julia"),
    ("kt", "kotlin"), ("kts", "kotlin"),
    ("less", "less"),
    ("lua", "lua"), ("luau", "lua"),
    ("markdown", "markdown"), ("md", "markdown"),
    ("mdx", "mdx"),
    ("nim", "nim"), ("nims", "nim"),
    ("ml", "ocaml"), ("mli", "ocaml"),
    ("pdf", "pdf"),
    ("php", "php"),
    ("prettier.config.cjs", "prettier"), ("prettier.config.js", "prettier"), ("prettier.config.mjs", "prettier"), ("prettierignore", "prettier"), ("prettierrc", "prettier"), ("prettierrc.js", "prettier"), ("prettierrc.json", "prettier"),
    ("prisma", "prisma"),
    ("py", "python"), ("pyw", "python"), ("ipynb", "python"),
    ("r", "r"), ("R", "r"),
    ("jsx", "reactjs"), ("cjsx", "reactjs"), ("mjsx", "reactjs"),
    ("tsx", "reactts"), ("ctsx", "reactts"), ("mtsx", "reactts"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("sass", "sass"), ("scss", "sass"),
    ("scala", "scala"), ("sc", "scala"),
    ("sh", "shell"), ("bash", "shell"), ("zsh", "shell"), ("fish", "shell"),
    ("ps1", "powershell"), ("psm1", "powershell"), ("psd1", "powershell"),
    ("sol", "solidity"),
    ("proto", "protobuf"),
    ("wasm", "wasm"),
    ("babelrc", "babel"),
    ("sql", "sql"), ("sqlite", "sql"), ("sqlite3", "sql"), ("db", "sql"),
    ("svelte", "svelte"),
    ("swift", "swift"),
    ("tf", "terraform"), ("tfvars", "terraform"),
    ("toml", "toml"),
    ("txt", "text"), ("text", "text"), ("log", "text"),
    ("cts", "typescript"), ("mts", "typescript"), ("ts", "typescript"),
    ("vue", "vue"),
    ("yaml", "yaml"), ("yml", "yaml"),
    ("zig", "zig"),
    ("zip", "zip"), ("tar", "zip"), ("gz", "zip"), ("7z", "zip"), ("rar", "zip"),
];

const TYPE_ICONS: &[(&str, &str)] = &[
    ("astro", "file_icons/file_type_astro.svg"),
    ("audio", "file_icons/file_type_audio.svg"),
    ("babel", "file_icons/file_type_babel.svg"),
    ("bat", "file_icons/file_type_bat.svg"),
    ("bun", "file_icons/file_type_bun.svg"),
    ("c", "file_icons/file_type_c.svg"),
    ("cargo", "file_icons/file_type_cargo.svg"),
    ("cpp", "file_icons/file_type_cpp.svg"),
    ("csharp", "file_icons/file_type_csharp.svg"),
    ("css", "file_icons/file_type_css.svg"),
    ("dart", "file_icons/file_type_dartlang.svg"),
    ("default", "file_icons/default_file.svg"),
    ("docker", "file_icons/file_type_docker.svg"),
    ("dotenv", "file_icons/file_type_dotenv.svg"),
    ("editorconfig", "file_icons/file_type_editorconfig.svg"),
    ("elixir", "file_icons/file_type_elixir.svg"),
    ("erlang", "file_icons/file_type_erlang.svg"),
    ("eslint", "file_icons/file_type_eslint.svg"),
    ("flutter", "file_icons/file_type_flutter.svg"),
    ("font", "file_icons/file_type_font.svg"),
    ("fsharp", "file_icons/file_type_fsharp.svg"),
    ("git", "file_icons/file_type_git.svg"),
    ("go", "file_icons/file_type_go.svg"),
    ("graphql", "file_icons/file_type_graphql.svg"),
    ("haskell", "file_icons/file_type_haskell.svg"),
    ("html", "file_icons/file_type_html.svg"),
    ("image", "file_icons/file_type_image.svg"),
    ("ini", "file_icons/file_type_ini.svg"),
    ("java", "file_icons/file_type_java.svg"),
    ("javascript", "file_icons/file_type_js.svg"),
    ("jsconfig", "file_icons/file_type_jsconfig.svg"),
    ("json", "file_icons/file_type_json.svg"),
    ("json5", "file_icons/file_type_json5.svg"),
    ("jsonld", "file_icons/file_type_jsonld.svg"),
    ("julia", "file_icons/file_type_julia.svg"),
    ("kotlin", "file_icons/file_type_kotlin.svg"),
    ("less", "file_icons/file_type_less.svg"),
    ("license", "file_icons/file_type_license.svg"),
    ("lua", "file_icons/file_type_lua.svg"),
    ("markdown", "file_icons/file_type_markdown.svg"),
    ("mdx", "file_icons/file_type_mdx.svg"),
    ("next", "file_icons/file_type_next.svg"),
    ("nim", "file_icons/file_type_nim.svg"),
    ("npm", "file_icons/file_type_npm.svg"),
    ("ocaml", "file_icons/file_type_ocaml.svg"),
    ("pdf", "file_icons/file_type_pdf.svg"),
    ("php", "file_icons/file_type_php.svg"),
    ("pnpm", "file_icons/file_type_pnpm.svg"),
    ("postcss", "file_icons/file_type_postcss.svg"),
    ("powershell", "file_icons/file_type_powershell.svg"),
    ("prettier", "file_icons/file_type_prettier.svg"),
    ("prisma", "file_icons/file_type_prisma.svg"),
    ("procfile", "file_icons/file_type_procfile.svg"),
    ("protobuf", "file_icons/file_type_protobuf.svg"),
    ("python", "file_icons/file_type_python.svg"),
    ("r", "file_icons/file_type_r.svg"),
    ("reactjs", "file_icons/file_type_reactjs.svg"),
    ("reactts", "file_icons/file_type_reactts.svg"),
    ("rollup", "file_icons/file_type_rollup.svg"),
    ("ruby", "file_icons/file_type_ruby.svg"),
    ("rust", "file_icons/file_type_rust.svg"),
    ("sass", "file_icons/file_type_sass.svg"),
    ("scala", "file_icons/file_type_scala.svg"),
    ("shell", "file_icons/file_type_shell.svg"),
    ("solidity", "file_icons/file_type_solidity.svg"),
    ("sql", "file_icons/file_type_sql.svg"),
    ("svelte", "file_icons/file_type_svelte.svg"),
    ("svg", "file_icons/file_type_svg.svg"),
    ("swift", "file_icons/file_type_swift.svg"),
    ("tailwind", "file_icons/file_type_tailwind.svg"),
    ("terraform", "file_icons/file_type_terraform.svg"),
    ("text", "file_icons/file_type_text.svg"),
    ("toml", "file_icons/file_type_toml.svg"),
    ("turbo", "file_icons/file_type_turbo.svg"),
    ("typescript", "file_icons/file_type_typescript.svg"),
    ("typescriptdef", "file_icons/file_type_typescriptdef.svg"),
    ("vite", "file_icons/file_type_vite.svg"),
    ("vue", "file_icons/file_type_vue.svg"),
    ("wasm", "file_icons/file_type_wasm.svg"),
    ("webpack", "file_icons/file_type_webpack.svg"),
    ("yaml", "file_icons/file_type_yaml.svg"),
    ("yarn", "file_icons/file_type_yarn.svg"),
    ("zig", "file_icons/file_type_zig.svg"),
    ("zip", "file_icons/file_type_zip.svg"),
];

fn stems() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| FILE_STEMS.iter().copied().collect())
}

fn suffixes() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| FILE_SUFFIXES.iter().copied().collect())
}

fn type_icons() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| TYPE_ICONS.iter().copied().collect())
}

fn icon_for_type(ty: &str) -> Option<&'static str> {
    type_icons().get(ty).copied()
}

fn type_for_suffix(suffix: &str) -> Option<&'static str> {
    stems()
        .get(suffix)
        .or_else(|| suffixes().get(suffix))
        .copied()
}

/// Returns the asset path of the official vscode-icons icon for a file path,
/// falling back to `default_file.svg`.
pub fn icon_for(path: &Path) -> &'static str {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return "file_icons/default_file.svg";
    };

    if name.starts_with(".env") || name.ends_with(".env") {
        return "file_icons/file_type_dotenv.svg";
    }

    let mut candidate = name;
    loop {
        if let Some(ty) = type_for_suffix(candidate) {
            if let Some(icon) = icon_for_type(ty) {
                return icon;
            }
        }
        match candidate.split_once('.') {
            Some((_, rest)) if !rest.is_empty() => candidate = rest,
            _ => break,
        }
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if let Some(icon) = type_for_suffix(ext).and_then(icon_for_type) {
            return icon;
        }
    }

    icon_for_type("default").unwrap_or("file_icons/default_file.svg")
}

pub const FOLDER_COLLAPSED: &str = "file_icons/default_folder.svg";
pub const FOLDER_EXPANDED: &str = "file_icons/default_folder_opened.svg";

pub fn folder_icon_for(path: &Path, expanded: bool) -> &'static str {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return if expanded { FOLDER_EXPANDED } else { FOLDER_COLLAPSED };
    };
    let clean = name.to_lowercase();
    match clean.as_str() {
        ".github" => if expanded { "file_icons/folder_type_github_opened.svg" } else { "file_icons/folder_type_github.svg" },
        ".git" | "git" => if expanded { "file_icons/folder_type_git_opened.svg" } else { "file_icons/folder_type_git.svg" },
        ".vscode" => if expanded { "file_icons/folder_type_vscode_opened.svg" } else { "file_icons/folder_type_vscode.svg" },
        ".husky" | "husky" => if expanded { "file_icons/folder_type_husky_opened.svg" } else { "file_icons/folder_type_husky.svg" },
        ".turbo" | "turbo" => if expanded { "file_icons/folder_type_turbo_opened.svg" } else { "file_icons/folder_type_turbo.svg" },
        "app" | "apps" => if expanded { "file_icons/folder_type_app_opened.svg" } else { "file_icons/folder_type_app.svg" },
        "docs" | "doc" | "documentation" => if expanded { "file_icons/folder_type_docs_opened.svg" } else { "file_icons/folder_type_docs.svg" },
        "src" | "source" | "sources" => if expanded { "file_icons/folder_type_src_opened.svg" } else { "file_icons/folder_type_src.svg" },
        "package" | "packages" => if expanded { "file_icons/folder_type_package_opened.svg" } else { "file_icons/folder_type_package.svg" },
        "asset" | "assets" => if expanded { "file_icons/folder_type_asset_opened.svg" } else { "file_icons/folder_type_asset.svg" },
        "image" | "images" | "img" | "icons" => if expanded { "file_icons/folder_type_images_opened.svg" } else { "file_icons/folder_type_images.svg" },
        "node_modules" => if expanded { "file_icons/folder_type_node_opened.svg" } else { "file_icons/folder_type_node.svg" },
        "script" | "scripts" => if expanded { "file_icons/folder_type_script_opened.svg" } else { "file_icons/folder_type_script.svg" },
        "server" => if expanded { "file_icons/folder_type_server_opened.svg" } else { "file_icons/folder_type_server.svg" },
        "web" | "www" | "public" => if expanded { "file_icons/folder_type_public_opened.svg" } else { "file_icons/folder_type_public.svg" },
        "test" | "tests" | "test-results" | "__tests__" | "spec" | "specs" => if expanded { "file_icons/folder_type_test_opened.svg" } else { "file_icons/folder_type_test.svg" },
        "theme" | "themes" | "style" | "styles" => if expanded { "file_icons/folder_type_theme_opened.svg" } else { "file_icons/folder_type_theme.svg" },
        "component" | "components" | "ui" => if expanded { "file_icons/folder_type_component_opened.svg" } else { "file_icons/folder_type_component.svg" },
        "api" | "apis" => if expanded { "file_icons/folder_type_api_opened.svg" } else { "file_icons/folder_type_api.svg" },
        "mobile" => if expanded { "file_icons/folder_type_mobile_opened.svg" } else { "file_icons/folder_type_mobile.svg" },
        "config" | "configs" | ".config" => if expanded { "file_icons/folder_type_config_opened.svg" } else { "file_icons/folder_type_config.svg" },
        "tools" | "utils" | "util" | "helpers" | "migration" | "migrations" => if expanded { "file_icons/folder_type_tools_opened.svg" } else { "file_icons/folder_type_tools.svg" },
        "view" | "views" | "pages" => if expanded { "file_icons/folder_type_view_opened.svg" } else { "file_icons/folder_type_view.svg" },
        "controller" | "controllers" => if expanded { "file_icons/folder_type_controller_opened.svg" } else { "file_icons/folder_type_controller.svg" },
        "model" | "models" => if expanded { "file_icons/folder_type_model_opened.svg" } else { "file_icons/folder_type_model.svg" },
        "middleware" | "middlewares" => if expanded { "file_icons/folder_type_middleware_opened.svg" } else { "file_icons/folder_type_middleware.svg" },
        "docker" | ".docker" => if expanded { "file_icons/folder_type_docker_opened.svg" } else { "file_icons/folder_type_docker.svg" },
        "font" | "fonts" => if expanded { "file_icons/folder_type_fonts_opened.svg" } else { "file_icons/folder_type_fonts.svg" },
        "plugin" | "plugins" => if expanded { "file_icons/folder_type_plugin_opened.svg" } else { "file_icons/folder_type_plugin.svg" },
        "dist" | "build" | "out" | "target" | ".next" => if expanded { "file_icons/folder_type_dist_opened.svg" } else { "file_icons/folder_type_dist.svg" },
        _ => if expanded { FOLDER_EXPANDED } else { FOLDER_COLLAPSED },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(name: &str) -> &'static str {
        icon_for(Path::new(name))
    }

    #[test]
    fn by_extension() {
        assert_eq!(icon("main.rs"), "file_icons/file_type_rust.svg");
        assert_eq!(icon("index.js"), "file_icons/file_type_js.svg");
        assert_eq!(icon("App.tsx"), "file_icons/file_type_reactts.svg");
        assert_eq!(icon("style.scss"), "file_icons/file_type_sass.svg");
        assert_eq!(icon("data.sql"), "file_icons/file_type_sql.svg");
        assert_eq!(icon("styled.d.ts"), "file_icons/file_type_typescriptdef.svg");
    }

    #[test]
    fn by_file_name() {
        assert_eq!(icon("Dockerfile"), "file_icons/file_type_docker.svg");
        assert_eq!(icon("Cargo.toml"), "file_icons/file_type_cargo.svg");
        assert_eq!(icon("package.json"), "file_icons/file_type_npm.svg");
    }

    #[test]
    fn env_files() {
        assert_eq!(icon(".env"), "file_icons/file_type_dotenv.svg");
        assert_eq!(icon(".env.local"), "file_icons/file_type_dotenv.svg");
        assert_eq!(icon(".env.example"), "file_icons/file_type_dotenv.svg");
        assert_eq!(icon(".env.test"), "file_icons/file_type_dotenv.svg");
        assert_eq!(icon(".env.twilio.template"), "file_icons/file_type_dotenv.svg");
    }

    #[test]
    fn config_files() {
        assert_eq!(icon("drizzle.config.ts"), "file_icons/file_type_typescript.svg");
        assert_eq!(icon("playwright.config.ts"), "file_icons/file_type_typescript.svg");
        assert_eq!(icon("commitlint.config.js"), "file_icons/file_type_js.svg");
        assert_eq!(icon("jest.config.js"), "file_icons/file_type_js.svg");
    }

    #[test]
    fn hidden_files() {
        assert_eq!(icon(".gitignore"), "file_icons/file_type_git.svg");
        assert_eq!(icon(".editorconfig"), "file_icons/file_type_editorconfig.svg");
        assert_eq!(icon(".dockerignore"), "file_icons/file_type_docker.svg");
        assert_eq!(icon(".npmrc"), "file_icons/file_type_npm.svg");
        assert_eq!(icon(".prettierignore"), "file_icons/file_type_prettier.svg");
    }

    #[test]
    fn multipart_names() {
        assert_eq!(icon("auth.module.js"), "file_icons/file_type_js.svg");
        assert_eq!(icon("Button.stories.tsx"), "file_icons/file_type_reactts.svg");
        assert_eq!(
            icon("eslint.config.mjs"),
            "file_icons/file_type_eslint.svg",
            "full-name suffix beats plain extension"
        );
    }

    #[test]
    fn fallback_default() {
        assert_eq!(icon("unknown.xyz"), "file_icons/default_file.svg");
    }

    #[test]
    fn all_folder_icons_exist_in_assets() {
        use crate::assets::AppAssets;
        let test_folders = [
            ".github", ".git", ".vscode", ".husky", ".turbo", "app", "apps",
            "docs", "src", "package", "assets", "images", "node_modules",
            "scripts", "server", "web", "public", "tests", "theme", "components",
            "api", "mobile", "config", "tools", "utils", "migrations",
            "views", "controllers", "models", "middleware", "docker", "fonts",
            "plugins", "dist", ".next", "unknown_folder"
        ];
        for f in test_folders {
            let closed = folder_icon_for(Path::new(f), false);
            let opened = folder_icon_for(Path::new(f), true);
            assert!(
                AppAssets::get(closed).is_some(),
                "Missing closed folder asset: {closed} for {f}"
            );
            assert!(
                AppAssets::get(opened).is_some(),
                "Missing opened folder asset: {opened} for {f}"
            );
        }
    }

    #[test]
    fn all_type_icons_exist_in_assets() {
        use crate::assets::AppAssets;
        for (ty, path) in TYPE_ICONS {
            assert!(
                AppAssets::get(path).is_some(),
                "Missing TYPE_ICON asset: {path} for {ty}"
            );
        }
    }
}
