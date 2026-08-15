use super::*;

#[test]
fn test_path_is_dev_nest_dev_path_returns_true() {
    let path = std::path::Path::new("/Users/someone/.buzz-dev");
    assert!(
        path_is_dev_nest(path),
        ".buzz-dev path must be identified as dev nest"
    );
}

#[test]
fn test_path_is_dev_nest_prod_path_returns_false() {
    let path = std::path::Path::new("/Users/someone/.buzz");
    assert!(
        !path_is_dev_nest(path),
        ".buzz path must not be identified as dev nest"
    );
}

#[test]
fn test_path_is_dev_nest_unrelated_path_returns_false() {
    let path = std::path::Path::new("/Users/someone/.buzz-staging");
    assert!(
        !path_is_dev_nest(path),
        "unrelated path must not be identified as dev nest"
    );
}

#[test]
fn test_path_is_dev_nest_root_returns_false() {
    let path = std::path::Path::new("/");
    assert!(
        !path_is_dev_nest(path),
        "root path must not be identified as dev nest"
    );
}
