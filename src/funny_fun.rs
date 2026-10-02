pub fn summm(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_summm() {
        assert!(summm(3, 3) == 6);
    }
}
