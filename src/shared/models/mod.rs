mod internal;

pub mod prelude {
    pub use super::internal::prelude::*;
}

#[cfg(test)]
pub(crate) fn parse_models_from_string(
    working_dir: &std::path::Path,
    file_path: &std::path::Path,
    input: &str,
) -> anyhow::Result<Vec<prelude::ParsedConfig>> {
    use serde_yaml::Deserializer;

    let mut models = Vec::new();
    for doc in Deserializer::from_str(input) {
        match crate::shared::config_load::parse_model(doc, working_dir, file_path) {
            Ok(Some(parsed_model)) => models.push(parsed_model.try_into()?),
            Ok(None) => {}
            Err(_) => break,
        }
    }

    Ok(models)
}
