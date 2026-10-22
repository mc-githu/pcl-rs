//! Modrinth API client (<https://docs.modrinth.com/api/>). No key required.

use anyhow::{Context, Result};
use pcl_net::{HttpClient, MODRINTH_API};
use serde::Deserialize;

use crate::types::{
    DepKind, Loader, ModDependency, ModFile, ModProject, ProjectSource, ReleaseType, SearchQuery,
    SearchResults,
};

pub struct Modrinth {
    client: HttpClient,
}

impl Modrinth {
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    pub async fn search(&self, query: &SearchQuery) -> Result<SearchResults> {
        // Scoped so the non-`Sync` query serializer never lives across an `.await`.
        let url = {
            let mut facets: Vec<Vec<String>> = vec![vec!["project_type:mod".to_string()]];
            if let Some(loader) = query.loader {
                facets.push(vec![format!("categories:{}", loader.as_str())]);
            }
            if let Some(version) = &query.game_version {
                facets.push(vec![format!("versions:{version}")]);
            }

            let mut params = url::form_urlencoded::Serializer::new(String::new());
            params.append_pair("query", &query.text);
            params.append_pair("limit", &query.limit.clamp(1, 100).to_string());
            params.append_pair("offset", &query.offset.to_string());
            params.append_pair("index", "relevance");
            params.append_pair("facets", &serde_json::to_string(&facets)?);
            format!("{MODRINTH_API}/search?{}", params.finish())
        };

        let response: SearchResponse = self.client.get_json(&url).await.context("Modrinth search")?;

        Ok(SearchResults {
            projects: response
                .hits
                .into_iter()
                .map(|hit| ModProject {
                    source: ProjectSource::Modrinth,
                    id: hit.project_id,
                    slug: hit.slug.clone(),
                    name: hit.title,
                    summary: hit.description,
                    author: hit.author,
                    downloads: hit.downloads,
                    icon_url: hit.icon_url,
                    url: format!("https://modrinth.com/mod/{}", hit.slug),
                })
                .collect(),
            total: response.total_hits,
            offset: response.offset,
        })
    }

    pub async fn project(&self, id_or_slug: &str) -> Result<ModProject> {
        let url = format!("{MODRINTH_API}/project/{id_or_slug}");
        let project: ProjectResponse = self
            .client
            .get_json(&url)
            .await
            .with_context(|| format!("Modrinth project {id_or_slug}"))?;

        Ok(ModProject {
            source: ProjectSource::Modrinth,
            id: project.id,
            slug: project.slug.clone(),
            name: project.title,
            summary: project.description,
            author: None,
            downloads: project.downloads,
            icon_url: project.icon_url,
            url: format!("https://modrinth.com/mod/{}", project.slug),
        })
    }

    /// List downloadable files for a project, newest first.
    pub async fn versions(
        &self,
        project: &str,
        game_version: Option<&str>,
        loader: Option<Loader>,
    ) -> Result<Vec<ModFile>> {
        let url = {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            if let Some(version) = game_version {
                params.append_pair("game_versions", &serde_json::to_string(&[version])?);
            }
            if let Some(loader) = loader {
                params.append_pair("loaders", &serde_json::to_string(&[loader.as_str()])?);
            }

            let query = params.finish();
            if query.is_empty() {
                format!("{MODRINTH_API}/project/{project}/version")
            } else {
                format!("{MODRINTH_API}/project/{project}/version?{query}")
            }
        };

        let versions: Vec<VersionResponse> = self
            .client
            .get_json(&url)
            .await
            .with_context(|| format!("Modrinth versions for {project}"))?;

        Ok(versions
            .into_iter()
            .map(|v| {
                let primary = v.files.iter().find(|f| f.primary).or_else(|| v.files.first());

                ModFile {
                    source: ProjectSource::Modrinth,
                    project_id: v.project_id,
                    version_id: v.id,
                    display_name: if v.name.is_empty() {
                        v.version_number
                    } else {
                        v.name
                    },
                    file_name: primary.map(|f| f.filename.clone()).unwrap_or_default(),
                    download_url: primary.map(|f| f.url.clone()),
                    game_versions: v.game_versions,
                    loaders: v.loaders,
                    release_type: match v.version_type.as_str() {
                        "alpha" => ReleaseType::Alpha,
                        "beta" => ReleaseType::Beta,
                        _ => ReleaseType::Release,
                    },
                    dependencies: v
                        .dependencies
                        .into_iter()
                        .map(|d| ModDependency {
                            kind: match d.dependency_type.as_str() {
                                "required" => DepKind::Required,
                                "incompatible" => DepKind::Incompatible,
                                "embedded" => DepKind::Embedded,
                                _ => DepKind::Optional,
                            },
                            project_id: d.project_id,
                            version_id: d.version_id,
                        })
                        .collect(),
                }
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    hits: Vec<SearchHit>,
    total_hits: u64,
    #[serde(default)]
    offset: u32,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    author: Option<String>,
    #[serde(default)]
    downloads: u64,
    icon_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectResponse {
    id: String,
    slug: String,
    title: String,
    description: String,
    #[serde(default)]
    downloads: u64,
    icon_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
    id: String,
    project_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version_number: String,
    #[serde(default = "default_release")]
    version_type: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    files: Vec<FileResponse>,
    #[serde(default)]
    dependencies: Vec<DependencyResponse>,
}

fn default_release() -> String {
    "release".to_string()
}

#[derive(Debug, Deserialize)]
struct FileResponse {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct DependencyResponse {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    dependency_type: String,
}
