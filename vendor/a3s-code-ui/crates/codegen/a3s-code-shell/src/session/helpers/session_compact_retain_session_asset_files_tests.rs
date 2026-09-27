use std::path::Path;

use super::{MAX_COMPACTION_IMAGE_PATHS, retain_session_asset_files};

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `count` regular files inside `assets`, named in chronological order.
fn write_assets(assets: &Path, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let file = assets.join(format!("image-{i:03}.png"));
            std::fs::write(&file, b"png").unwrap();
            path_string(&file)
        })
        .collect()
}

#[tokio::test]
async fn keeps_the_newest_assets_up_to_the_cap() {
    let assets = tempfile::tempdir().unwrap();
    let real = write_assets(assets.path(), MAX_COMPACTION_IMAGE_PATHS + 1);

    let (kept, dropped) = retain_session_asset_files(real.clone(), assets.path()).await;

    assert_eq!(kept, real.get(1..).unwrap_or_default());
    assert_eq!(dropped, 1);
}

/// Junk newer than the real assets must not use up the cap.
#[tokio::test]
async fn newer_junk_does_not_consume_the_cap() {
    let assets = tempfile::tempdir().unwrap();
    let real = write_assets(assets.path(), 5);
    let junk: Vec<String> = (0..37)
        .map(|i| path_string(&assets.path().join(format!("gone-{i}.png"))))
        .chain(["/etc/passwd", "/etc/hostname", "/etc/hosts"].map(str::to_owned))
        .collect();
    let paths: Vec<String> = real.iter().cloned().chain(junk).collect();

    let (kept, dropped) = retain_session_asset_files(paths, assets.path()).await;

    assert_eq!(kept, real);
    assert_eq!(dropped, 40);
}

#[tokio::test]
async fn keeps_only_regular_files_inside_the_assets_dir() {
    let assets = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let inside = assets.path().join("image-1.png");
    std::fs::write(&inside, b"png").unwrap();
    let outside = elsewhere.path().join("image-2.png");
    std::fs::write(&outside, b"png").unwrap();
    let link_inside = assets.path().join("link-inside.png");
    std::os::unix::fs::symlink(&inside, &link_inside).unwrap();
    let link_outside = assets.path().join("link-outside.png");
    std::os::unix::fs::symlink(&outside, &link_outside).unwrap();
    let directory = assets.path().join("nested");
    std::fs::create_dir(&directory).unwrap();
    let missing = assets.path().join("gone.png");
    let nested_dir = assets.path().join("sub");
    std::fs::create_dir(&nested_dir).unwrap();
    let nested = nested_dir.join("x.png");
    std::fs::write(&nested, b"png").unwrap();
    // A symlinked subdirectory would make an outside file look like it sits under `assets/`.
    let link_dir = assets.path().join("link");
    std::os::unix::fs::symlink(elsewhere.path(), &link_dir).unwrap();
    let through_link_dir = link_dir.join("image-2.png");
    // Resolves into the assets dir, but only through `..`.
    let assets_name = assets.path().file_name().expect("tempdir has a file name");
    let dotdot = assets
        .path()
        .join("..")
        .join(assets_name)
        .join("image-1.png");

    let (kept, dropped) = retain_session_asset_files(
        vec![
            path_string(&inside),
            path_string(&outside),
            path_string(&link_inside),
            path_string(&link_outside),
            path_string(&directory),
            path_string(&missing),
            path_string(&dotdot),
            path_string(&nested),
            path_string(&through_link_dir),
            "relative/image-1.png".to_owned(),
        ],
        assets.path(),
    )
    .await;

    assert_eq!(kept, vec![path_string(&inside)]);
    assert_eq!(dropped, 9);
}
