use common::ServiceInfo;

const INFO: ServiceInfo = ServiceInfo::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

fn main() {
    println!("{} starting", INFO.banner());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_is_wired_to_package_metadata() {
        assert_eq!(INFO.name, "api");
        assert!(!INFO.version.is_empty());
    }
}
