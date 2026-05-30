-- Sample SQL file to exercise syntax highlighting, indents,
-- and folds in vorto. Open with `vorto assets/samples/hello.sql`.

CREATE TABLE person (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    age        INTEGER NOT NULL CHECK (age >= 0),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE tag (
    person_id INTEGER NOT NULL REFERENCES person (id),
    label     TEXT NOT NULL,
    PRIMARY KEY (person_id, label)
);

INSERT INTO person (id, name, age) VALUES
    (1, 'Alice', 30),
    (2, 'Bob', 17),
    (3, 'Carol', 42);

INSERT INTO tag (person_id, label) VALUES
    (1, 'admin'),
    (1, 'early_bird'),
    (3, 'admin');

SELECT
    p.name,
    p.age,
    CASE
        WHEN p.age >= 18 THEN 'adult'
        ELSE 'minor'
    END AS group_label,
    COUNT(t.label) AS tag_count
FROM person AS p
LEFT JOIN tag AS t ON t.person_id = p.id
WHERE p.age BETWEEN 18 AND 99
GROUP BY p.name, p.age
HAVING COUNT(t.label) >= 0
ORDER BY p.age DESC;
