use ascurra_harness::http_client::{self, Collection, Config, Request};

#[test]
fn project_timeout_is_shared_and_persisted() {
    let mut config = Config::default();
    config.timeout_seconds = 17;
    config.collections.push(Collection {
        id: "collection".into(),
        name: "Tests".into(),
        exposed: true,
        requests: vec![
            Request {
                id: "first".into(),
                url: "http://localhost/first".into(),
                ..Request::default()
            },
            Request {
                id: "second".into(),
                url: "http://localhost/second".into(),
                ..Request::default()
            },
        ],
    });
    for agent in [false, true] {
        for id in ["first", "second"] {
            assert_eq!(
                http_client::prepare(&config, "collection", id, agent)
                    .unwrap()
                    .timeout_seconds,
                17
            );
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    http_client::save(temp.path(), &project, &config).unwrap();
    assert_eq!(
        http_client::load(temp.path(), &project)
            .unwrap()
            .timeout_seconds,
        17
    );
    let json = serde_json::to_value(&config).unwrap();
    assert!(
        json["collections"][0]["requests"][0]
            .get("timeout_seconds")
            .is_none()
    );
    for timeout in [0, 121] {
        config.timeout_seconds = timeout;
        assert!(http_client::validate(&config).is_err());
    }
}

#[test]
fn legacy_request_timeouts_do_not_override_the_project_default() {
    let config: Config = serde_json::from_str(r#"{
        "variables": {},
        "collections": [{"id":"collection","name":"Tests","requests":[
            {"id":"legacy","name":"Old","method":"GET","url":"http://localhost/","timeout_seconds":5}
        ]}]
    }"#).unwrap();
    assert_eq!(config.timeout_seconds, 30);
    assert_eq!(
        http_client::prepare(&config, "collection", "legacy", false)
            .unwrap()
            .timeout_seconds,
        30
    );
}
