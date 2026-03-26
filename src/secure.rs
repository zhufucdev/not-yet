use rand::{RngExt, rngs::StdRng};

pub fn generate_random_id(len: usize) -> String {
    let mut rng: StdRng = rand::make_rng();
    const DICT: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
    String::from_utf8(
        (0..=len)
            .map(|_| DICT[rng.random::<i32>() as usize % DICT.len()])
            .collect(),
    )
    .unwrap()
}

pub fn generate_content_boundary() -> String {
    format!("--ContentBoundary {}", generate_random_id(32))
}
