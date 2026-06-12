fn main() {
    let raw = r#"sh -c "echo '{\"decision\":\"deny\"}'""#;
    let tokens = shlex::split(raw).unwrap();
    println!("{:?}", tokens);
}
