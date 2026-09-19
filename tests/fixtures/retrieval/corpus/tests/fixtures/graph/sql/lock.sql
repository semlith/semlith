CREATE TABLE lock_holder (
  id INTEGER PRIMARY KEY
);

CREATE VIEW acquire AS
SELECT id FROM lock_holder;
