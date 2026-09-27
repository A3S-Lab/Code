use super::*;

fn snapshot(families: &[Option<&str>]) -> TaskModelCatalogSnapshot {
    TaskModelCatalogSnapshot {
        eligible: families
            .iter()
            .enumerate()
            .map(|(index, family)| EligibleTaskModel {
                id: format!("model-{index}"),
                model_family: family.map(str::to_owned),
            })
            .collect(),
        authority: CatalogAuthority::Complete,
    }
}

#[test]
fn selection_is_hidden_only_for_an_enabled_all_a3s_catalog() {
    let all_a3s = || snapshot(&[Some("a3s"), Some("a3s")]);
    assert_eq!(
        TaskModelSelection::Inherited,
        resolve_presentation(true, all_a3s()).selection
    );
    assert_eq!(
        TaskModelSelection::Selectable,
        resolve_presentation(true, snapshot(&[Some("a3s"), None])).selection
    );
    assert_eq!(
        TaskModelSelection::Selectable,
        resolve_presentation(false, all_a3s()).selection
    );
    let mut provisional = all_a3s();
    provisional.authority = CatalogAuthority::Provisional;
    assert_eq!(
        TaskModelSelection::Selectable,
        resolve_presentation(true, provisional).selection
    );
}
