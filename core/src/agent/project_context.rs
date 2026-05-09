/// Inspect the workspace for well-known project marker files and return a short
/// `## Project Context` section that the agent can use without any manual configuration.
/// Returns an empty string when the workspace type cannot be determined.
pub(super) fn detect_project_hint(workspace: &std::path::Path) -> String {
    struct Marker {
        file: &'static str,
        lang: &'static str,
        tip: &'static str,
    }

    let markers = [
        Marker {
            file: "Cargo.toml",
            lang: "Rust",
            tip: "Use `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt`. \
              Prefer `anyhow` / `thiserror` for error handling. \
              Follow the Microsoft Rust Guidelines (no panics in library code, \
              async-first with Tokio).",
        },
        Marker {
            file: "package.json",
            lang: "Node.js / TypeScript",
            tip: "Check `package.json` for the package manager (npm/yarn/pnpm/bun) \
              and available scripts. Prefer TypeScript with strict mode. \
              Use ESM imports unless the project is CommonJS.",
        },
        Marker {
            file: "pyproject.toml",
            lang: "Python",
            tip: "Use the package manager declared in `pyproject.toml` \
              (uv, poetry, hatch, etc.). Prefer type hints and async/await for I/O.",
        },
        Marker {
            file: "setup.py",
            lang: "Python",
            tip: "Legacy Python project. Prefer type hints and async/await for I/O.",
        },
        Marker {
            file: "requirements.txt",
            lang: "Python",
            tip: "Python project with pip-style dependencies. \
              Prefer type hints and async/await for I/O.",
        },
        Marker {
            file: "go.mod",
            lang: "Go",
            tip: "Use `go build ./...` and `go test ./...`. \
              Follow standard Go project layout. Use `gofmt` for formatting.",
        },
        Marker {
            file: "pom.xml",
            lang: "Java / Maven",
            tip: "Use `mvn compile`, `mvn test`, `mvn package`. \
              Follow standard Maven project structure.",
        },
        Marker {
            file: "build.gradle",
            lang: "Java / Gradle",
            tip: "Use `./gradlew build` and `./gradlew test`. \
              Follow standard Gradle project structure.",
        },
        Marker {
            file: "build.gradle.kts",
            lang: "Kotlin / Gradle",
            tip: "Use `./gradlew build` and `./gradlew test`. \
              Prefer Kotlin coroutines for async work.",
        },
        Marker {
            file: "CMakeLists.txt",
            lang: "C / C++",
            tip: "Use `cmake -B build && cmake --build build`. \
              Check for `compile_commands.json` for IDE tooling.",
        },
        Marker {
            file: "Makefile",
            lang: "C / C++ (or generic)",
            tip: "Use `make` or `make <target>`. \
              Check available targets with `make help` or by reading the Makefile.",
        },
    ];

    // Check for C# / .NET — no single fixed filename, so glob for *.csproj / *.sln
    let is_dotnet = workspace.join("*.csproj").exists() || {
        // Fast check: look for any .csproj or .sln in the workspace root
        std::fs::read_dir(workspace)
            .map(|entries| {
                entries.flatten().any(|e| {
                    let name = e.file_name();
                    let s = name.to_string_lossy();
                    s.ends_with(".csproj") || s.ends_with(".sln")
                })
            })
            .unwrap_or(false)
    };

    if is_dotnet {
        return "## Project Context\n\nThis is a **C# / .NET** project. \
         Use `dotnet build`, `dotnet test`, and `dotnet run`. \
         Follow C# coding conventions and async/await patterns."
            .to_string();
    }

    for marker in &markers {
        if workspace.join(marker.file).exists() {
            return format!(
                "## Project Context\n\nThis is a **{}** project. {}",
                marker.lang, marker.tip
            );
        }
    }

    String::new()
}
