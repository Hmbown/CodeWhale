use std::path::Path;
fn main() {
    let p1 = Path::new(r"C:\data\protocols\union-of-experts");
    let p2 = Path::new(r"c:\data\protocols\union-of-experts");
    
    let c1 = std::fs::canonicalize(p1).unwrap();
    let c2 = std::fs::canonicalize(p2).unwrap();
    
    println!("c1: {:?}", c1);
    println!("c2: {:?}", c2);
    println!("c1 == c2: {}", c1 == c2);
}
