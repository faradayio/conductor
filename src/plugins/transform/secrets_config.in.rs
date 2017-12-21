// This is not a standalone Rust module.  It gets processed by serde to
// generate serialization code and included directly into another module.

/// The secrets for a single service.  We implement this as a very thin
/// wrapper around `BTreeMap` so that we can add methods.
#[derive(Default, Debug, PartialEq, Eq)]
struct ServiceSecrets {
    secrets: BTreeMap<String, String>,
}

impl ServiceSecrets {
    fn to_compose_env(&self) -> BTreeMap<String, Option<dc::RawOr<String>>> {
        let mut env = BTreeMap::new();
        for (var, val) in &self.secrets {
            let val = dc::escape(val).expect("escape string should never fail");
            env.insert(var.to_owned(), Some(val));
        }
        env
    }
}

impl<'de> Deserialize<'de> for ServiceSecrets {
    fn deserialize<D>(deserializer: D) -> result::Result<Self, D::Error>
        where D: Deserializer<'de>,
    {
        let secrets = Deserialize::deserialize(deserializer)?;
        Ok(ServiceSecrets { secrets: secrets })
    }
}

impl Serialize for ServiceSecrets {
    fn serialize<S>(&self, serializer: S) -> result::Result<S::Ok, S::Error>
        where S: Serializer
    {
        self.secrets.serialize(serializer)
    }
}

/// The secrets for a pod.
type PodSecrets = BTreeMap<String, ServiceSecrets>;

/// The secrets for an target.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetSecrets {
    /// Shared between all services in this target.
    #[serde(default)]
    common: ServiceSecrets,
    /// Secrets for each of our pods.
    #[serde(default)]
    pods: BTreeMap<String, PodSecrets>,
}

/// The deserialized form of `secrets.yml`.  This is basically
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// Shared between all services in this pod.
    #[serde(default)]
    common: ServiceSecrets,
    /// Secrets for each of our pods.
    #[serde(default)]
    pods: BTreeMap<String, PodSecrets>,
    /// Secrets for each of our targets.
    #[serde(default)]
    targets: BTreeMap<String, TargetSecrets>,
}

#[test]
fn can_deserialize_config() {
    let path = Path::new("examples/rails_hello/config/secrets.yml");
    let config: Config = load_yaml(path).unwrap();
    assert_eq!(config.common.secrets.get("GLOBAL_PASSWORD").unwrap(), "magic");
}
