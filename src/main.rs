use lazy_static::lazy_static;

// We use boxes for errors because this is a simple binary where performance does not matter and
// errors are rare.
type Result_<T> = std::result::Result<T, Box<std::error::Error>>;
type ScoreSaberSongId = u64;
// Initially this was [u8; 20] because the hash is 160 bits but it is easier to keep it as an
// opaque string because we are never doing any operation directly on the hash.
type SongHash = String;

const DATABASE_PATH: &str = "beatsaber.sqlite";
const DATABASE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS "scoresaber_songs" (
    "uid" INTEGER NOT NULL UNIQUE,
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "songSubName" TEXT NOT NULL,
    "songAuthorName" TEXT NOT NULL,
    "levelAuthorName" TEXT NOT NULL,
    "bpm" INTEGER NOT NULL,
    "diff" TEXT NOT NULL,
    "stars" REAL NOT NULL,
    PRIMARY KEY("uid")
);
"#;

const SCORESABER_API_URL: &str = "https://scoresaber.com/api.php";

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
struct ScoreSaberSong {
    uid: ScoreSaberSongId,
    #[serde(rename = "id")]
    id: SongHash,
    name: String,
    #[serde(rename = "songSubName")]
    sub_name: String,
    #[serde(rename = "songAuthorName")]
    song_author: String,
    #[serde(rename = "levelAuthorName")]
    level_author: String,
    #[serde(rename = "bpm")]
    beats_per_minute: u64,
    #[serde(rename = "diff")]
    difficulty: String,
    #[serde(rename = "stars")]
    star_difficulty: f64,
}

struct RankedSongsPage<T: Iterator<Item = ScoreSaberSong>> {
    songs: T,
    last_page: bool,
}

fn extract_ranked_songs_page<T: std::io::Read>(
    response: T,
    limit: usize,
) -> Result_<RankedSongsPage<impl Iterator<Item = ScoreSaberSong>>> {
    #[derive(Clone, Debug, PartialEq, serde::Deserialize)]
    struct Songs {
        songs: Vec<ScoreSaberSong>,
    }
    let songs: Songs = serde_json::from_reader(response)?;
    let last = songs.songs.len() < limit;
    Ok(RankedSongsPage {
        songs: songs.songs.into_iter(),
        last_page: last,
    })
}

// 1 is first page
fn get_ranked_songs_page(
    client: &reqwest::Client,
    page: u64,
) -> Result_<RankedSongsPage<impl Iterator<Item = ScoreSaberSong>>> {
    // cat=1 means sort by date ranked
    const LIMIT: usize = 1000;
    let url = reqwest::Url::parse_with_params(
        SCORESABER_API_URL,
        &[
            ("function", "get-leaderboards"),
            ("ranked", "1"),
            ("cat", "1"),
            ("limit", &LIMIT.to_string()),
            ("page", &page.to_string()),
        ],
    )?;
    log::info!("request: {}", url);
    let response = client.get(url).send()?;
    if response.status().is_success() {
        extract_ranked_songs_page(response, LIMIT)
    } else {
        Err(format!(
            "response status code does not indiciate success: {}",
            response.status()
        ))?
    }
}

fn get_ranked_songs(
    client: &reqwest::Client,
) -> impl Iterator<Item = Result_<ScoreSaberSong>> + '_ {
    struct Iter<'a> {
        // TODO: this type should be exactly the result of get_ranked_songs_page which is
        // `impl Iterator`. However we cannot use impl in a struct and I failed to express the same
        // thing using generics.
        songs: Box<Iterator<Item = ScoreSaberSong>>,
        next_page: Option<u64>,
        client: &'a reqwest::Client,
    }

    impl<'a> Iterator for Iter<'a> {
        type Item = Result_<ScoreSaberSong>;

        fn next(&mut self) -> Option<Self::Item> {
            match self.songs.next() {
                Some(song) => Some(Ok(song)),
                None => {
                    match self.next_page {
                        Some(page) => {
                            match get_ranked_songs_page(self.client, page) {
                                Ok(response) => {
                                    self.songs = Box::new(response.songs);
                                    // Increment current_page only after adding the songs to the vector. This way if
                                    // retrieving the response fails, the state is unchanged.
                                    self.next_page = match response.last_page {
                                        true => None,
                                        false => Some(page + 1),
                                    };
                                    self.next()
                                }
                                Err(err) => Some(Err(err)),
                            }
                        }
                        None => None,
                    }
                }
            }
        }
    }

    Iter {
        songs: Box::new(vec![].into_iter()),
        next_page: Some(1),
        client: client,
    }
}

fn insert_song_into_db(db: &rusqlite::Connection, song: &ScoreSaberSong) -> Result_<()> {
    let mut insert_statement = db.prepare("REPLACE INTO scoresaber_songs (uid, id, name, songSubName, songAuthorName, levelAuthorName, bpm, diff, stars) VALUES (?,?,?,?,?,?,?,?,?)")?;
    let rows_affected = insert_statement.execute(rusqlite::params![
        song.uid as i64,
        song.id,
        song.name,
        song.sub_name,
        song.song_author,
        song.level_author,
        song.beats_per_minute as i64,
        song.difficulty,
        song.star_difficulty
    ])?;
    if rows_affected != 1 {
        return Err("rows_affected is not 1")?;
    }
    Ok(())
}

fn scrape_all_songs(db: &rusqlite::Connection) -> Result_<()> {
    let client = reqwest::Client::new();
    for (i, song_result) in get_ranked_songs(&client).enumerate() {
        let song = song_result?;
        println!(
            "handling song number {} with id {} and name {}",
            i, song.uid, song.name
        );
        insert_song_into_db(db, &song)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct BeatSaberPlaylistSong {
    #[serde(rename = "songName")]
    name: String,
    #[serde(rename = "hash")]
    hash: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct BeatsaberPlaylist {
    #[serde(rename = "playlistTitle")]
    title: String,
    #[serde(rename = "playlistAuthor")]
    author: String,
    #[serde(rename = "playlistDescription")]
    description: String,
    #[serde(rename = "songs")]
    songs: Vec<BeatSaberPlaylistSong>,
}

fn make_beatsaber_playlist(db: &rusqlite::Connection) -> Result_<BeatsaberPlaylist> {
    const TITLE: &str = "Ranked Songs";
    const AUTHOR: &str = "Valentin (e00E)";
    const DESCRIPTION: &str = "Contains all songs that are ranked on Score Saber ordered by star difficulty (roughly equivalent to maximum PP) in descending order.";
    // GROUP_BY and MAX(stars) are needed because the same hash is part of multiple difficulties of
    // the same song so we sort by the maximum of all difficulties.
    let mut statement =
        db.prepare("SELECT id,name FROM scoresaber_songs GROUP BY id ORDER BY MAX(stars) DESC")?;

    let mut playlist = BeatsaberPlaylist {
        title: TITLE.to_string(),
        author: AUTHOR.to_string(),
        description: DESCRIPTION.to_string(),
        songs: vec![],
    };

    struct Song {
        hash: String,
        name: String,
    }
    let iter = statement.query_map(rusqlite::params![], |row| {
        Ok(Song {
            hash: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    for song_result in iter {
        let song = song_result?;
        playlist.songs.push(BeatSaberPlaylistSong {
            name: song.name,
            hash: song.hash,
        });
    }
    Ok(playlist)
}

fn save_beatsaber_playlist(playlist: BeatsaberPlaylist) -> Result_<()> {
    let file = std::fs::File::create("ranked_songs.json")?;
    serde_json::to_writer_pretty(file, &playlist)?;
    println!("Used {} songs in playlist.", playlist.songs.len());
    Ok(())
}

fn main() -> Result_<()> {
    env_logger::init();
    let db = rusqlite::Connection::open(DATABASE_PATH)?;
    db.execute(DATABASE_SCHEMA, rusqlite::params![])?;
    scrape_all_songs(&db)?;
    save_beatsaber_playlist(make_beatsaber_playlist(&db)?)?;
    db.close().map_err(|x| x.1.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    lazy_static! {
        static ref SONGS: [ScoreSaberSong; 4] = [
            ScoreSaberSong {
                uid: 101208,
                id: "7719B8DE597CB1BFDFD6048E5FC51656DD5219EE".to_string(),
                name:
                    "Happppy song -- other difficulty that does not really exist just for the test"
                        .to_string(),
                sub_name: "".to_string(),
                song_author: "SOOOO".to_string(),
                level_author: "Hexagonial".to_string(),
                beats_per_minute: 226,
                difficulty: "_ExpertPlus_SoloStandard".to_string(),
                star_difficulty: 1.0,
            },
            ScoreSaberSong {
                uid: 101208,
                id: "7719B8DE597CB1BFDFD6048E5FC51656DD5219EE".to_string(),
                name: "Happppy song".to_string(),
                sub_name: "".to_string(),
                song_author: "SOOOO".to_string(),
                level_author: "Hexagonial".to_string(),
                beats_per_minute: 226,
                difficulty: "_ExpertPlus_SoloStandard".to_string(),
                star_difficulty: 9.72,
            },
            ScoreSaberSong {
                uid: 109086,
                id: "CFCA2FE00BCC418DC9ECF64D92FC01CEEC52C375".to_string(),
                name: "Milk Crown on Sonnetica".to_string(),
                sub_name: "".to_string(),
                song_author: "nameless".to_string(),
                level_author: "Hexagonial".to_string(),
                beats_per_minute: 255,
                difficulty: "_ExpertPlus_SoloStandard".to_string(),
                star_difficulty: 10.08,
            },
            ScoreSaberSong {
                uid: 100024,
                id: "762B7BF1C06DBCC7AAB23D955A553E5420FBA6E5".to_string(),
                name: "NUCLEAR-STAR".to_string(),
                sub_name: "".to_string(),
                song_author: "Camellia".to_string(),
                level_author: "Hexagonial".to_string(),
                beats_per_minute: 199,
                difficulty: "_ExpertPlus_SoloStandard".to_string(),
                star_difficulty: 9.38,
            },
        ];
    }

    #[test]
    fn test_extract_ranked_songs_page() {
        let result =
            extract_ranked_songs_page(&include_bytes!("../test_data/get-leaderboards.json")[..], 3)
                .unwrap();
        assert_eq!(result.last_page, false);
        assert_eq!(result.songs.collect::<Vec<ScoreSaberSong>>()[..], SONGS[..]);
    }

    #[test]
    fn test_into_database_to_playlist() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute(DATABASE_SCHEMA, rusqlite::params![]).unwrap();
        for song in SONGS.iter() {
            insert_song_into_db(&db, song).unwrap();
        }
        let playlist = make_beatsaber_playlist(&db).unwrap();
        // Remove first song because it is lower difficulty duplicate of second.
        let mut expected_songs = SONGS[1..].to_owned();
        expected_songs.sort_by(|x, y| y.star_difficulty.partial_cmp(&x.star_difficulty).unwrap());
        let expected_playlist = expected_songs
            .iter()
            .map(|x| BeatSaberPlaylistSong {
                name: x.name.clone(),
                hash: x.id.clone(),
            })
            .collect::<Vec<BeatSaberPlaylistSong>>();
        assert_eq!(playlist.songs, expected_playlist);
        db.close().unwrap();
    }
}
