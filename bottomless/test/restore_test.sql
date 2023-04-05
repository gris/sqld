.bail on
.echo on
.load ../../target/debug/bottomless
.open file:test.db?wal=bottomless
.mode column
SELECT v, length(v) FROM test;
