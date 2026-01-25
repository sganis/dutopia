// rs/src/bin/duapi/index.rs
use anyhow::{Context, Result};
use csv::ReaderBuilder;
use memchr::memchr_iter;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Age {
    pub count: u64,
    pub size: u64,
    pub disk: u64,
    pub linked: u64,
    pub atime: i64,
    pub mtime: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FolderOut {
    pub path: String,
    pub users: HashMap<String, HashMap<String, Age>>,
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub file_count: u64,
    pub file_size: u64,
    pub disk_bytes: u64,
    pub linked_size: u64,
    pub latest_atime: i64,
    pub latest_mtime: i64,
}

#[derive(Debug, Clone)]
pub struct TrieNode {
    pub children: HashMap<String, Box<TrieNode>>,
    pub users: HashSet<String>,
}

impl TrieNode {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            users: HashSet::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryFSIndex {
    root: TrieNode,
    pub total_entries: usize,
    per_user_age: HashMap<(String, String, u8), Stats>,
    users_by_path: HashMap<String, HashSet<String>>,
}

impl InMemoryFSIndex {
    pub fn new() -> Self {
        Self {
            root: TrieNode::new(),
            total_entries: 0,
            per_user_age: HashMap::new(),
            users_by_path: HashMap::new(),
        }
    }

    pub fn load_from_csv(&mut self, path: &Path) -> Result<Vec<String>> {
        print!("Counting lines in {}... ", path.display());
        std::io::stdout().flush().unwrap();
        let total_lines = count_lines(path)?;
        let data_lines = total_lines.saturating_sub(1);
        let progress_interval = if data_lines >= 10 {
            data_lines / 10
        } else {
            0
        };
        println!("done:\nTotal lines: {}", total_lines);
        println!("Loading and building index...");

        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .from_path(path)
            .with_context(|| format!("Failed to open CSV file: {}", path.display()))?;

        let mut all_users: HashSet<String> = HashSet::new();
        let mut loaded_count = 0usize;

        for (line_no, record) in rdr.records().enumerate() {
            let record =
                record.with_context(|| format!("Failed to read CSV line {}", line_no + 2))?;
            if record.len() < 9 {
                continue;
            }

            let path_str = record.get(0).unwrap_or("");
            let username = record.get(1).unwrap_or("").trim().to_string();
            let age: u8 = record.get(2).unwrap_or("0").parse().unwrap_or(0);
            let file_count: u64 = record.get(3).unwrap_or("0").parse().unwrap_or(0);
            let file_size: u64 = record.get(4).unwrap_or("0").parse().unwrap_or(0);
            let disk_bytes: u64 = record.get(5).unwrap_or("0").parse().unwrap_or(0);
            let linked_size: u64 = record.get(6).unwrap_or("0").parse().unwrap_or(0);
            let latest_atime: i64 = record.get(7).unwrap_or("0").parse().unwrap_or(0);
            let latest_mtime: i64 = record.get(8).unwrap_or("0").parse().unwrap_or(0);

            if path_str.is_empty() || username.is_empty() {
                continue;
            }

            all_users.insert(username.clone());

            let pkey = Self::canonical_key(path_str);
            self.users_by_path
                .entry(pkey.clone())
                .or_default()
                .insert(username.clone());

            self.insert_path(path_str, &username);

            let entry = self
                .per_user_age
                .entry((pkey, username, age))
                .or_insert_with(Stats::default);
            entry.file_count = entry.file_count.saturating_add(file_count);
            entry.file_size = entry.file_size.saturating_add(file_size);
            entry.disk_bytes = entry.disk_bytes.saturating_add(disk_bytes);
            entry.linked_size = entry.linked_size.saturating_add(linked_size);
            if latest_atime > entry.latest_atime {
                entry.latest_atime = latest_atime;
            }
            if latest_mtime > entry.latest_mtime {
                entry.latest_mtime = latest_mtime;
            }

            loaded_count += 1;
            if progress_interval > 0 && (line_no + 1) % progress_interval == 0 {
                let percent =
                    ((line_no + 1) as f64 * 100.0 / data_lines.max(1) as f64).ceil() as u32;
                println!("{}%", percent.min(100));
            }
        }

        self.total_entries = loaded_count;

        let mut users: Vec<String> = all_users.into_iter().collect();
        users.sort();
        Ok(users)
    }

    fn insert_path(&mut self, path: &str, username: &str) {
        let components = Self::path_to_components(path);
        let mut current = &mut self.root;
        for component in components {
            current = current
                .children
                .entry(component)
                .or_insert_with(|| Box::new(TrieNode::new()));
            current.users.insert(username.to_string());
        }
    }

    pub fn list_children(
        &self,
        dir_path: &str,
        user_filter: &Vec<String>,
        age_filter: Option<u8>,
    ) -> Result<Vec<FolderOut>> {
        let components = Self::path_to_components(dir_path);
        let mut current = &self.root;
        for component in components {
            current = current
                .children
                .get(&component)
                .ok_or_else(|| anyhow::anyhow!("Directory not found: {}", dir_path))?
                .as_ref();
        }

        let mut items = Vec::new();
        let base_path = Self::normalize_path(dir_path);

        for (child_name, _child_node) in &current.children {
            let full_path = if base_path.is_empty() || base_path == "/" {
                format!("/{}", child_name)
            } else {
                format!("{}/{}", base_path.trim_end_matches('/'), child_name)
            };

            let pkey = Self::canonical_key(&full_path);

            let available_users = self.users_by_path.get(&pkey);
            if available_users.is_none() {
                continue;
            }
            let available_users = available_users.unwrap();

            let mut users_to_show: Vec<String> = if user_filter.is_empty() {
                available_users.iter().cloned().collect()
            } else {
                available_users
                    .iter()
                    .filter(|u| user_filter.contains(*u))
                    .cloned()
                    .collect()
            };
            users_to_show.sort();

            if !user_filter.is_empty() && users_to_show.is_empty() {
                continue;
            }

            let mut users_map: HashMap<String, HashMap<String, Age>> = HashMap::new();
            let ages_to_consider: Vec<u8> = if let Some(a) = age_filter {
                vec![a]
            } else {
                vec![0, 1, 2]
            };

            for uname in &users_to_show {
                let mut age_map: HashMap<String, Age> = HashMap::new();

                for a in &ages_to_consider {
                    if let Some(s) = self.per_user_age.get(&(pkey.clone(), uname.clone(), *a)) {
                        age_map.insert(
                            a.to_string(),
                            Age {
                                count: s.file_count,
                                size: s.file_size,
                                disk: s.disk_bytes,
                                linked: s.linked_size,
                                atime: s.latest_atime,
                                mtime: s.latest_mtime,
                            },
                        );
                    }
                }

                if !age_map.is_empty() {
                    users_map.insert(uname.clone(), age_map);
                }
            }

            if users_map.is_empty() {
                continue;
            }

            items.push(FolderOut {
                path: full_path,
                users: users_map,
            });
        }

        items.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(items)
    }

    pub fn path_to_components(path: &str) -> Vec<String> {
        let normalized = Self::normalize_path(path);
        normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    pub fn normalize_path(path: &str) -> String {
        let mut normalized = path.replace('\\', "/");
        if cfg!(windows) && normalized.len() >= 2 && normalized.chars().nth(1) == Some(':') {
            if !normalized.starts_with('/') {
                normalized = format!("/{}", normalized);
            }
        } else if !normalized.starts_with('/') && !normalized.is_empty() {
            normalized = format!("/{}", normalized);
        }
        normalized
    }

    pub fn canonical_key(path: &str) -> String {
        let mut n = Self::normalize_path(path);
        if n.len() > 1 {
            n = n.trim_end_matches('/').to_string();
        }
        n
    }
}

pub fn count_lines(path: &Path) -> Result<usize> {
    let mut file = File::open(path)?;
    let mut buf = [0u8; 128 * 1024];
    let mut count = 0usize;
    let mut last: Option<u8> = None;

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += memchr_iter(b'\n', &buf[..n]).count();
        last = Some(buf[n - 1]);
    }

    if let Some(b) = last {
        if b != b'\n' {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_count_lines_with_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "a\nb\n").unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 2);
    }

    #[test]
    fn test_count_lines_without_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "a\nb").unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 2);
    }

    #[test]
    fn test_count_lines_empty() {
        let f = NamedTempFile::new().unwrap();
        assert_eq!(count_lines(f.path()).unwrap(), 0);
    }

    #[test]
    fn test_normalize_and_canonical() {
        assert_eq!(InMemoryFSIndex::normalize_path("foo/bar"), "/foo/bar");
        assert_eq!(InMemoryFSIndex::canonical_key("/foo/bar/"), "/foo/bar");
        assert_eq!(
            InMemoryFSIndex::path_to_components("/a/b/c"),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn test_normalize_path_root() {
        assert_eq!(InMemoryFSIndex::normalize_path("/"), "/");
        assert_eq!(InMemoryFSIndex::canonical_key("/"), "/");
    }

    #[test]
    fn test_path_to_components_root() {
        let comps = InMemoryFSIndex::path_to_components("/");
        assert!(comps.is_empty());
    }

    #[test]
    fn test_stats_default() {
        let stats = Stats::default();
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.file_size, 0);
        assert_eq!(stats.disk_bytes, 0);
        assert_eq!(stats.linked_size, 0);
        assert_eq!(stats.latest_atime, 0);
        assert_eq!(stats.latest_mtime, 0);
    }

    #[test]
    fn test_stats_accumulation_with_linked() {
        let mut stats = Stats::default();

        stats.file_count = 1;
        stats.file_size = 1000;
        stats.disk_bytes = 1000;
        stats.linked_size = 0;

        stats.file_count += 1;
        stats.file_size += 1000;
        stats.disk_bytes += 0;
        stats.linked_size += 1000;

        assert_eq!(stats.file_count, 2);
        assert_eq!(stats.file_size, 2000);
        assert_eq!(stats.disk_bytes, 1000);
        assert_eq!(stats.linked_size, 1000);
    }

    #[test]
    fn test_trie_node_new() {
        let node = TrieNode::new();
        assert!(node.children.is_empty());
        assert!(node.users.is_empty());
    }

    #[test]
    fn test_inmemory_index_new() {
        let idx = InMemoryFSIndex::new();
        assert_eq!(idx.total_entries, 0);
    }

    #[test]
    fn test_load_from_csv() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,atime,mtime\n\
             /,alice,0,2,200,100,0,1700000000,1700000100\n\
             /,bob,1,1,50,50,0,1600000000,1600000100\n\
             /docs,alice,2,3,600,300,300,1500000000,1500000050"
        )
        .unwrap();

        let mut idx = InMemoryFSIndex::new();
        let users = idx.load_from_csv(f.path()).unwrap();

        assert!(users.contains(&"alice".to_string()));
        assert!(users.contains(&"bob".to_string()));
        assert_eq!(idx.total_entries, 3);
    }

    #[test]
    fn test_list_children() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,atime,mtime\n\
             /,alice,0,2,200,100,0,1700000000,1700000100\n\
             /docs,alice,2,3,600,300,300,1500000000,1500000050"
        )
        .unwrap();

        let mut idx = InMemoryFSIndex::new();
        idx.load_from_csv(f.path()).unwrap();

        let items = idx.list_children("/", &Vec::new(), None).unwrap();
        assert!(items.iter().any(|it| it.path == "/docs"));
    }

    #[test]
    fn test_list_children_with_user_filter() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,atime,mtime\n\
             /docs,alice,0,2,200,100,0,1700000000,1700000100\n\
             /docs,bob,0,1,50,50,0,1600000000,1600000100"
        )
        .unwrap();

        let mut idx = InMemoryFSIndex::new();
        idx.load_from_csv(f.path()).unwrap();

        let items = idx
            .list_children("/", &vec!["alice".into()], None)
            .unwrap();
        assert_eq!(items.len(), 1);
        let docs = &items[0];
        assert!(docs.users.contains_key("alice"));
        assert!(!docs.users.contains_key("bob"));
    }

    #[test]
    fn test_list_children_with_age_filter() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "path,user,age,files,size,disk,linked,atime,mtime\n\
             /docs,alice,0,2,200,100,0,1700000000,1700000100\n\
             /docs,alice,2,3,600,300,300,1500000000,1500000050"
        )
        .unwrap();

        let mut idx = InMemoryFSIndex::new();
        idx.load_from_csv(f.path()).unwrap();

        let items = idx.list_children("/", &Vec::new(), Some(2)).unwrap();
        let docs = &items[0];
        let alice_ages = docs.users.get("alice").unwrap();
        assert!(alice_ages.contains_key("2"));
        assert!(!alice_ages.contains_key("0"));
    }
}
