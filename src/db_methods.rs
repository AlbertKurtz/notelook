use turso::Connection;

pub async fn add_note(text: String, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();

    let rows_affected = conn
        .execute("INSERT INTO notes (content) VALUES (?1)", (text,))
        .await?;

    println!("affected {rows_affected} rows");

    Ok(())
}

pub async fn note_by_id(id: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let mut row_by_id = conn
        .query(
            "SELECT id, timestamp, content FROM notes where id=(?1)",
            (id,),
        )
        .await?;
    while let Some(row) = row_by_id.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn delete_note(id: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let _ = conn
        .execute("DELETE FROM notes where id=(?1)", (id,))
        .await?;

    Ok(())
}

pub async fn all_notes(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    // let text: String = "A random note to you =),  second one lol!".to_string();
    let mut current_rows = conn
        .query("SELECT id, timestamp, content FROM notes", ())
        .await?;
    while let Some(row) = current_rows.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn last_notes(last_n: i32, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let sql = "
            SELECT id, timestamp, content
            FROM (
                SELECT id, timestamp, content
                FROM notes
                ORDER BY id DESC
                LIMIT ?1
            )
            ORDER BY id ASC
        ";
    let mut current_rows = conn.query(sql, (last_n,)).await?;
    while let Some(row) = current_rows.next().await? {
        let id: i32 = row.get(0)?;
        let timestamp: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let ts_display = timestamp.as_deref().unwrap_or("no timestamp");
        println!("{id} [{ts_display}]: {content}");
    }

    Ok(())
}

pub async fn add_label(
    label: &str,
    note_id: i32,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let sql = "
            INSERT INTO labels (label, note_id) VALUES (?1, ?2)
        ";

    let new_label = conn.execute(sql, (label, note_id)).await?;
    // print!("{new_label}");

    Ok(())
}
