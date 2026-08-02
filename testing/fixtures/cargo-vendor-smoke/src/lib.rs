pub fn render(value: u64) -> String {
    itoa::Buffer::new().format(value).to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_through_the_external_dependency() {
        assert_eq!(super::render(42), "42");
    }
}
