use a3s_acl::{AttributeSchema, BlockSchema, Cardinality, Schema, ValueSchema};

pub(crate) fn release_schema() -> Schema {
    let artifact = Schema::new()
        .attribute("digest", AttributeSchema::required(ValueSchema::string()))
        .attribute(
            "media_type",
            AttributeSchema::required(ValueSchema::string()),
        );
    let entrypoint = Schema::new()
        .attribute("command", AttributeSchema::required(ValueSchema::string()))
        .attribute(
            "args",
            AttributeSchema::required(ValueSchema::list(ValueSchema::string())),
        );
    let health = Schema::new()
        .attribute(
            "transport",
            AttributeSchema::required(ValueSchema::string()),
        )
        .attribute("port", AttributeSchema::required(ValueSchema::number()))
        .attribute(
            "readiness_path",
            AttributeSchema::required(ValueSchema::string()),
        )
        .attribute(
            "liveness_path",
            AttributeSchema::required(ValueSchema::string()),
        )
        .attribute(
            "shutdown_grace_seconds",
            AttributeSchema::required(ValueSchema::number()),
        );
    let storage = Schema::new()
        .attribute(
            "workspace",
            AttributeSchema::required(ValueSchema::string()),
        )
        .attribute("cache", AttributeSchema::required(ValueSchema::string()))
        .attribute(
            "persistent_data",
            AttributeSchema::required(ValueSchema::string()),
        );
    let capability =
        Schema::new().attribute("level", AttributeSchema::required(ValueSchema::number()));
    let secret = Schema::new()
        .attribute("target", AttributeSchema::required(ValueSchema::string()))
        .attribute(
            "destination",
            AttributeSchema::required(ValueSchema::string()),
        );
    let provenance = Schema::new()
        .attribute("uri", AttributeSchema::required(ValueSchema::string()))
        .attribute("digest", AttributeSchema::required(ValueSchema::string()));
    let release = Schema::new()
        .attribute("schema", AttributeSchema::required(ValueSchema::string()))
        .attribute("protocol", AttributeSchema::required(ValueSchema::string()))
        .block(
            "artifact",
            BlockSchema::new(artifact).occurrences(Cardinality::exactly(1)),
        )
        .block(
            "entrypoint",
            BlockSchema::new(entrypoint).occurrences(Cardinality::exactly(1)),
        )
        .block(
            "health",
            BlockSchema::new(health).occurrences(Cardinality::exactly(1)),
        )
        .block(
            "storage",
            BlockSchema::new(storage).occurrences(Cardinality::exactly(1)),
        )
        .block(
            "capability",
            BlockSchema::new(capability)
                .occurrences(Cardinality::at_least(1))
                .labels(Cardinality::exactly(1))
                .unordered(true),
        )
        .block(
            "secret",
            BlockSchema::new(secret)
                .occurrences(Cardinality::at_least(0))
                .labels(Cardinality::exactly(1))
                .unordered(true),
        )
        .block(
            "provenance",
            BlockSchema::new(provenance)
                .occurrences(Cardinality::at_least(1))
                .labels(Cardinality::exactly(1))
                .unordered(true),
        );
    Schema::new().block(
        "agent_release",
        BlockSchema::new(release).occurrences(Cardinality::exactly(1)),
    )
}
