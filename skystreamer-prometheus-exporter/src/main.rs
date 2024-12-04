use color_eyre::Result;
use futures::StreamExt;
use prometheus_exporter::{self, prometheus::register_int_counter};
use skystreamer::{stream::PostStream, RepoSubscription};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

fn default_level_filter() -> LevelFilter {
    #[cfg(debug_assertions)]
    return LevelFilter::DEBUG;
    #[cfg(not(debug_assertions))]
    return LevelFilter::INFO;
}

/// Tries to normalize a language code to its main language code
#[tracing::instrument]
fn handle_language(lang: &str) -> Option<String> {
    // for some reason, langtag::Language::new("jp") is still a valid language
    // shouldn't it be converted to "ja"?
    // wtf?
    let special_cases = [/* ("jp", "ja"), */ ("angika", "anp")];
    let lang = special_cases
        .iter()
        .find_map(|(from, to)| {
            if lang.to_lowercase() == *from {
                Some(to)
            } else {
                None
            }
        })
        .map_or(lang, |v| v);

    // tracing::debug!(?lang, "lang_tag");
    let lang = langtag::LangTag::new(lang).ok()?.language()?;

    // let lang_tag = langtag::LangTag::new(lang).ok()?;
    // println!("{:?}", lang_tag.as_normal());
    let primary = lang.primary();
    let subtags = lang.extension_subtags().collect::<Vec<_>>();
    tracing::trace!(?subtags, ?lang, ?primary, "lang_tag");
    Some(primary.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let env_filter = EnvFilter::builder()
        .with_default_directive(default_level_filter().into())
        .from_env()?;

    color_eyre::install()?;

    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .with_level(true)
        .with_file(false)
        .compact()
        .with_line_number(false)
        .with_env_filter(env_filter)
        .init();

    let max_sample_size = std::env::var("MAX_SAMPLE_SIZE")
        .map(|val| val.parse::<usize>().unwrap_or(10000))
        .ok();

    let normalize_langs = std::env::var("NORMALIZE_LANGS")
        .map(|val| val.parse::<bool>().unwrap_or(true))
        .ok();

    tracing::info!("Starting skystreamer-prometheus-exporter");
    tracing::info!("MAX_SAMPLE_SIZE: {:?}", max_sample_size);

    let binding = "0.0.0.0:9100".parse()?;
    let _exporter = prometheus_exporter::start(binding)?;
    let counter = register_int_counter!(
        "skystreamer_bsky_posts",
        "Number of posts from bsky.network"
    )?;

    let language_counter = prometheus_exporter::prometheus::register_int_counter_vec!(
        "skystreamer_bsky_posts_by_language_grouped",
        "Number of posts from bsky.network by language",
        &["language"]
    )?;

    let language_counter_individual = prometheus_exporter::prometheus::register_int_counter_vec!(
        "skystreamer_bsky_posts_by_language",
        "Number of posts from bsky.network by language (individually)",
        &["language"]
    )?;

    // const MAX_SAMPLE_SIZE: usize = 10000;

    loop {
        let subscription = RepoSubscription::new("bsky.network").await.unwrap();
        let post_stream = PostStream::new(subscription);
        let mut post_stream = post_stream.await;
        let stream = post_stream.stream().await?;

        futures::pin_mut!(stream);
        // let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        // interval.tick().await;

        // let mut last_tick = tokio::time::Instant::now();

        while let Some(post) = stream.next().await {
            counter.inc();

            // post.language_raw.iter().for_each(|lang| {
            //     // Let's normalize all the languages to its main language
            //     let binding = lang.as_ref();
            //     let a = binding.language();
            //     tracing::info!("lang: {:?}", a);

            //     // let lang_norm = handle_language(lang).unwrap_or_else(|| lang.to_lowercase());
            //     // language_counter_individual
            //     //     .with_label_values(&[&lang_norm])
            //     //     .inc();
            // });

            let langs = post
                .language
                .iter()
                .map(|lang| {
                    // Let's normalize all the languages to its main language

                    // let lang_norm = handle_language(lang).unwrap_or_else(|| lang.to_lowercase());
                    let processed_language = if lang.is_empty() {
                        "null".to_string()
                    } else if normalize_langs.unwrap_or(true) {
                        let l = handle_language(lang);
                        if l.is_none() {
                            tracing::warn!("Failed to normalize language: {}", lang);
                        }
                        l.unwrap_or_else(|| lang.to_lowercase())
                    } else {
                        lang.to_string()
                    };

                    language_counter_individual
                        .with_label_values(&[&processed_language])
                        .inc();

                    processed_language
                })
                .collect::<Vec<_>>();

            langs.iter().for_each(|lang| {
                language_counter_individual.with_label_values(&[lang]).inc();
            });

            let langs_joined = if langs.is_empty() {
                "null".to_string()
            } else {
                langs.join(",")
            };
            language_counter.with_label_values(&[&langs_joined]).inc();
            // handle for grouped languages

            if let Some(max_size) = max_sample_size {
                if counter.get() > max_size as u64 {
                    counter.reset();
                }
            }
        }
    }

    // Ok(())
}

#[cfg(test)]

mod tests {
    use super::*;
    use tracing_test::traced_test;

    #[test]
    #[traced_test]
    fn test_basic_normalization() {
        assert_eq!(handle_language("en"), Some("en".to_string()));
        assert_eq!(handle_language("en"), Some("en".to_string()));
    }
    // Test alternate shortcodes, like jp for ja (Japanese)
    #[test]
    #[traced_test]
    fn test_alt_language_normalization() {
        assert_eq!(handle_language("jp"), Some("ja".to_string()));
    }

    #[test]
    #[traced_test]
    fn test_subscript_normalization() {
        assert_eq!(handle_language("en-US"), Some("en".to_string()));
        assert_eq!(handle_language("en-GB"), Some("en".to_string()));
    }

    #[test]
    #[traced_test]
    /// Serializing weird Indian codes (i.e Angika)
    ///
    /// ATProto does 0 checking on the language codes, so we need to normalize them for analytics
    fn test_obscure_in_normalization() {
        assert_eq!(handle_language("Angika"), Some("anp".to_string()));
    }

    #[test]
    #[traced_test]
    fn test_null_normalization() {
        assert_eq!(handle_language(""), None);
    }
}
