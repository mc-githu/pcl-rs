//! CurseForge API client (<https://docs.curseforge.com/>).
//!
//! CurseForge requires an API key, obtainable free of charge from
//! <https://console.curseforge.com/>. PCL-RS never ships one: the user supplies it
//! and requests go straight to the official API.
//!
//! Files whose authors disabled third-party distribution come back with a null
//! `downloadUrl`; those are surfaced as manual downloads instead of being fetched
//! from a mirror.

use anyhow::{bail, Context, Result};
use pcl_net::{HttpClient, CURSEFORGE_API};
use serde::Deserialize;

use crate::types::{
    DepKind, Loader, ModDependency, ModFile, ModProject, ProjectSource, ReleaseType, SearchQuery,
    SearchResults,
};

/// Minecraft's `gameId` on CurseForge.
const GAME_ID: u32 = 432;
/// `classId` for mods.
const CLASS_MODS: u32 = 6;

pub struct CurseForge {
    client: HttpClient,
    api_key: String,
}

impl CurseForge {
    pub fn new(client: HttpClient, api_key: impl Into<String>) -> Self {
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    fn headers(&self) -> Vec<(&str, &str)> {
        vec![("x-api-key", self.api_key.as_str()), ("accept", "application/json")]
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        if !self.has_api_key() {
            bail!(
                "CurseForge 需要 API Key：请在设置中填写，或在 https://console.curseforge.com/ 免费申请"
            );
        }
        self.client
            .get_json_with_headers::<T>(url, &self.headers())
            .await
            .with_context(|| format!("CurseForge GET {url}"))
    }

    pub async fn search(&self, query: &SearchQuery) -> Result<SearchResults> {
        let mut params = url::form_urlencoded::Serializer::new(String::new());
        params.append_pair("gameId", &GAME_ID.to_string());
        params.append_pair("classId", &CLASS_MODS.to_string());
        params.append_pair("searchFilter", &query.text);
        params.append_pair("index", &query.offset.to_string());
        params.append_pair("pageSize", &query.limit.clamp(1, 50).to_string());
        params.append_pair("sortField", "2");
        params.append_pair("sortOrder", "desc");
        if let Some(version) = &query.game_version {
            params.append_pair("gameVersion", version);
        }
        if let Some(loader) = query.loader {
            if loader != Loader::Vanilla {
                params.append_pair("modLoaderType", &loader.curseforge_id().to_string());
            }
        }

        let url = format!("{CURSEFORGE_API}/mods/search?{}", params.finish());
        let response: Paginated<Vec<ModResponse>> = self.get(&url).await?;

        Ok(SearchResults {
            projects: response.data.into_iter().map(ModResponse::into_project).collect(),
            total: response.pagination.map(|p| p.total_count).unwrap_or(0),
            offset: query.offset,
        })
    }

    pub async fn project(&self, mod_id: &str) -> Result<ModProject> {
        let url = format!("{CURSEFORGE_API}/mods/{mod_id}");
        let response: Wrapper<ModResponse> = self.get(&url).await?;
        Ok(response.data.into_project())
    }

    /// List files for a project, filtered by game version and loader.
    pub async fn files(
        &self,
        mod_id: &str,
        game_version: Option<&str>,
        loader: Option<Loader>,
    ) -> Result<Vec<ModFile>> {
        let mut params = url::form_urlencoded::Serializer::new(String::new());
        params.append_pair("index", "0");
        params.append_pair("pageSize", "50");
        if let Some(version) = game_version {
            params.append_pair("gameVersion", version);
        }
        if let Some(loader) = loader {
            if loader != Loader::Vanilla {
                params.append_pair("modLoaderType", &loader.curseforge_id().to_string());
            }
        }

        let url = format!("{CURSEFORGE_API}/mods/{mod_id}/files?{}", params.finish());
        let response: Wrapper<Vec<FileResponse>> = self.get(&url).await?;

        Ok(response
            .data
            .into_iter()
            .map(|f| ModFile {
                source: ProjectSource::CurseForge,
                project_id: mod_id.to_string(),
                version_id: f.id.to_string(),
                display_name: f.display_name.clone(),
                file_name: f.file_name,
                download_url: f.download_url.filter(|u| !u.is_empty()),
                game_versions: f.game_versions,
                loaders: Vec::new(),
                release_type: match f.release_type {
                    2 => ReleaseType::Beta,
                    3 => ReleaseType::Alpha,
                    _ => ReleaseType::Release,
                },
                dependencies: f
                    .dependencies
                    .into_iter()
                    .map(|d| ModDependency {
                        kind: match d.relation_type {
                            3 => DepKind::Required,
                            2 => DepKind::Optional,
                            5 => DepKind::Incompatible,
                            1 | 6 => DepKind::Embedded,
                            _ => DepKind::Optional,
                        },
                        project_id: Some(d.mod_id.to_string()),
                        version_id: None,
                    })
                    .collect(),
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
struct Wrapper<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct Paginated<T> {
    data: T,
    #[serde(default)]
    pagination: Option<Pagination>,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(rename = "totalCount", default)]
    total_count: u64,
}

#[derive(Debug, Deserialize)]
struct ModResponse {
    id: u64,
    name: String,
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(rename = "downloadCount", default)]
    download_count: u64,
    #[serde(default)]
    links: Option<ModLinks>,
    #[serde(default)]
    logo: Option<ModLogo>,
    #[serde(default)]
    authors: Vec<ModAuthor>,
}

impl ModResponse {
    fn into_project(self) -> ModProject {
        ModProject {
            source: ProjectSource::CurseForge,
            id: self.id.to_string(),
            slug: self.slug.clone(),
            name: self.name,
            summary: self.summary,
            author: self.authors.first().map(|a| a.name.clone()),
            downloads: self.download_count,
            icon_url: self.logo.and_then(|l| l.thumbnail_url),
            url: self.links.and_then(|l| l.website_url).unwrap_or_else(|| {
                format!("https://www.curseforge.com/minecraft/mc-mods/{}", self.slug)
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModLinks {
    #[serde(rename = "websiteUrl", default)]
    website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModLogo {
    #[serde(rename = "thumbnailUrl", default)]
    thumbnail_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModAuthor {
    name: String,
}

#[derive(Debug, Deserialize)]
struct FileResponse {
    id: u64,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(rename = "fileName", default)]
    file_name: String,
    #[serde(rename = "downloadUrl", default)]
    download_url: Option<String>,
    #[serde(rename = "gameVersions", default)]
    game_versions: Vec<String>,
    #[serde(rename = "releaseType", default)]
    release_type: u32,
    #[serde(default)]
    dependencies: Vec<FileDependency>,
}

#[derive(Debug, Deserialize)]
struct FileDependency {
    #[serde(rename = "modId", default)]
    mod_id: u64,
    #[serde(rename = "relationType", default)]
    relation_type: u32,
}
