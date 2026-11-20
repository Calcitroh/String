// A stand-in for Navidrome's database, for the self-test's phone-listening checks: only the tables and columns String
// reads (library, media_file, scrobbles), in WAL mode like the real one. Never a real one.
//   node tests\fake-navidrome.js <db file> <test media folder>
// Plays, all in the last few minutes: Glass Orbit twice, Low Tide once, and a song String doesn't have.
'use strict';
const fs = require('fs'), path = require('path');
const { DatabaseSync } = require('node:sqlite');
const [file, media] = process.argv.slice(2);
if (!file || !media) { console.log('usage: node tests\\fake-navidrome.js <db file> <test media folder>'); process.exit(1); }
fs.mkdirSync(path.dirname(file), { recursive: true });
const db = new DatabaseSync(file);
db.exec(`PRAGMA journal_mode = WAL;
  CREATE TABLE user (id TEXT PRIMARY KEY, user_name TEXT);
  INSERT INTO user VALUES ('u1', 'Fixture listener'), ('u2', 'Another listener');
  CREATE TABLE library (id INTEGER PRIMARY KEY, name TEXT, path TEXT);
  CREATE TABLE media_file (id VARCHAR(255) PRIMARY KEY, path TEXT, title TEXT, album TEXT, artist TEXT, duration REAL, library_id INTEGER);
  CREATE TABLE scrobbles (id INTEGER PRIMARY KEY, media_file_id VARCHAR(255) NOT NULL, user_id VARCHAR(255) NOT NULL, submission_time INTEGER NOT NULL);`);
db.prepare('INSERT INTO library (id, name, path) VALUES (1, ?, ?)').run('Music Library', media);
const song = db.prepare('INSERT INTO media_file (id, path, title, album, artist, duration, library_id) VALUES (?, ?, ?, ?, ?, ?, 1)');
song.run('m1', 'glass orbit.mp3', 'Navidrome Title', 'Synthetic Weather', 'Test Signal', 24);
song.run('m2', 'low tide.flac', 'Low Tide', 'Synthetic Weather', '', 18);
song.run('m3', 'Gone/old song.mp3', 'Old Song', '', 'Someone Else', 200);
const now = Math.floor(Date.now() / 1000), play = db.prepare('INSERT INTO scrobbles (media_file_id, user_id, submission_time) VALUES (?, ?, ?)');
play.run('m1', 'u1', now - 240);
play.run('m2', 'u1', now - 180);
play.run('m1', 'u1', now - 120);
play.run('m3', 'u1', now - 60);
play.run('m3', 'u2', now - 30);
db.close();
console.log('fake Navidrome database: ' + file);
