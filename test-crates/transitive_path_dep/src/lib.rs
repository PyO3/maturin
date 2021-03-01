pub fn is_sum(x: usize, y: usize, sum: usize) -> bool {
    x + y == sum
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
