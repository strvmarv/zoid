mod claude;
mod classify;
mod emit;
mod fetch;

fn main() {
    eprintln!("zoid-plugin-import: use `bulk <marketplace.json>` or `repo <owner/name[/subpath]>`");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
