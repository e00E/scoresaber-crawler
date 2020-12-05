# ScoreSaber Crawler

This application extracts all ranked songs from [ScoreSaber](https://scoresaber.com/). The songs are stored in a sqlite database for further processing and a playlist is created which contains them in descending order of *star difficulty* which correlates roughly to maximum achievable performance points.

The database (`beatsaber.sqlite`) and the playlist (`ranked_songs.json`) are part of the repository so that they can be used without running the program.