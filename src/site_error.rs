use axum::body::Body;
use axum::http::{
    HeaderMap, Response, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
};

pub(crate) fn prefers_html_error_page(headers: &HeaderMap) -> bool {
    let wants_document = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            let value = value.to_ascii_lowercase();
            matches!(value.as_str(), "document" | "iframe")
        })
        .unwrap_or(false);

    if wants_document {
        return true;
    }

    headers
        .get("accept")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("text/html") || value.contains("application/xhtml+xml")
        })
        .unwrap_or(false)
}

pub(crate) fn summarize_error_detail(content_type: Option<&str>, bytes: &[u8]) -> Option<String> {
    let body = String::from_utf8_lossy(bytes);
    if body.trim().is_empty() {
        return None;
    }

    if error_body_looks_like_html(content_type, bytes) {
        let stripped = decode_html_entities(&collapse_whitespace(&strip_html_tags(&body)));
        if let Some(detail) = extract_gateway_path_error(&stripped) {
            return Some(truncate_text(&detail));
        }

        if let Some(title) = extract_html_title(&body) {
            return Some(truncate_text(&title));
        }

        if stripped.is_empty() {
            return Some("Gateway returned HTML error page instead of site content.".to_string());
        }

        return Some(truncate_text(&stripped));
    }

    let collapsed = collapse_whitespace(&body);
    if collapsed.is_empty() {
        None
    } else {
        Some(truncate_text(&collapsed))
    }
}

pub(crate) fn error_body_looks_like_html(content_type: Option<&str>, bytes: &[u8]) -> bool {
    let body = String::from_utf8_lossy(bytes);
    looks_like_html_error(content_type, &body)
}

pub(crate) fn build_site_error_response(
    status: StatusCode,
    wants_html: bool,
    title: &str,
    summary: &str,
    detail: Option<&str>,
    host: &str,
    path: &str,
) -> Response<Body> {
    if wants_html {
        build_html_error_response(status, title, summary, detail, host, path)
    } else {
        build_text_error_response(status, title, summary, detail, host, path)
    }
}

fn build_html_error_response(
    status: StatusCode,
    title: &str,
    summary: &str,
    detail: Option<&str>,
    host: &str,
    path: &str,
) -> Response<Body> {
    let status_text = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Error")
    );
    let path = if path.is_empty() { "/" } else { path };
    let request_target = format!("https://{host}{path}");
    let detail_block = detail
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            format!(
                r#"
        <article class="vapor-subtle-panel meta-card detail-card">
          <p class="meta-label">Detail</p>
          <p class="meta-copy">{}</p>
        </article>"#,
                escape_html(value.trim())
            )
        })
        .unwrap_or_default();
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="dark light">
  <title>{title} | NeoMist</title>
  <style>
    :root {{
      color-scheme: light;
      --primary: #6f6cf6;
      --secondary: #9d86f7;
      --accent: #53c7e8;
      --error: #ef6b8f;
      --warning: #f2a65a;
      --base-100: #ffffff;
      --base-200: #f7f8fc;
      --base-300: #e7e9f3;
      --base-content: #27304a;
      --muted: rgba(39, 48, 74, 0.65);
      --soft: rgba(39, 48, 74, 0.52);
      --panel-bg: rgba(255, 255, 255, 0.82);
      --subtle-bg: rgba(255, 255, 255, 0.72);
      --panel-shadow: 0 18px 50px rgba(111, 108, 246, 0.06), inset 0 1px 0 rgba(255, 255, 255, 0.45);
      --subtle-shadow: 0 10px 30px rgba(111, 108, 246, 0.04), inset 0 1px 0 rgba(255, 255, 255, 0.38);
      --button-shadow: 0 10px 24px rgba(111, 108, 246, 0.18), inset 0 1px 0 rgba(255, 255, 255, 0.24);
    }}

    @media (prefers-color-scheme: dark) {{
      :root {{
        color-scheme: dark;
        --primary: #8f8cff;
        --secondary: #be9cff;
        --accent: #67d8f4;
        --error: #ff819f;
        --warning: #f3b474;
        --base-100: #161b2e;
        --base-200: #1d243b;
        --base-300: #313a57;
        --base-content: #eef2ff;
        --muted: rgba(238, 242, 255, 0.7);
        --soft: rgba(238, 242, 255, 0.52);
        --panel-bg: rgba(22, 27, 46, 0.82);
        --subtle-bg: rgba(22, 27, 46, 0.7);
        --panel-shadow: 0 18px 50px rgba(143, 140, 255, 0.06), inset 0 1px 0 rgba(255, 255, 255, 0.08);
        --subtle-shadow: 0 10px 30px rgba(143, 140, 255, 0.04), inset 0 1px 0 rgba(255, 255, 255, 0.08);
        --button-shadow: 0 10px 24px rgba(143, 140, 255, 0.18), inset 0 1px 0 rgba(255, 255, 255, 0.12);
      }}
    }}

    * {{ box-sizing: border-box; }}

    html {{
      min-height: 100%;
      background:
        radial-gradient(48% 42% at 12% 8%, rgba(111, 108, 246, 0.07), transparent 62%),
        radial-gradient(34% 30% at 88% 12%, rgba(83, 199, 232, 0.08), transparent 68%),
        linear-gradient(180deg, var(--base-100), var(--base-200) 100%);
      background-attachment: fixed;
    }}

    @media (prefers-color-scheme: dark) {{
      html {{
        background:
          radial-gradient(48% 42% at 12% 8%, rgba(143, 140, 255, 0.07), transparent 62%),
          radial-gradient(34% 30% at 88% 12%, rgba(103, 216, 244, 0.08), transparent 68%),
          linear-gradient(180deg, var(--base-100), var(--base-200) 100%);
      }}
    }}

    body {{
      margin: 0;
      min-height: 100vh;
      color: var(--base-content);
      background: transparent;
      text-rendering: optimizeLegibility;
      -webkit-font-smoothing: antialiased;
      font: 16px/1.5 Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }}

    body::before {{
      content: "";
      position: fixed;
      inset: 0;
      background:
        radial-gradient(36% 30% at 50% 10%, rgba(111, 108, 246, 0.06), transparent 70%),
        radial-gradient(30% 26% at 88% 24%, rgba(83, 199, 232, 0.05), transparent 72%);
      z-index: -1;
    }}

    @media (prefers-color-scheme: dark) {{
      body::before {{
        background:
          radial-gradient(36% 30% at 50% 10%, rgba(143, 140, 255, 0.06), transparent 70%),
          radial-gradient(30% 26% at 88% 24%, rgba(103, 216, 244, 0.05), transparent 72%);
      }}
    }}

    .vapor-shell {{
      min-height: 100vh;
      padding: 16px;
    }}

    .vapor-frame {{
      width: min(980px, 100%);
      margin: 0 auto;
      min-height: calc(100vh - 32px);
      display: flex;
      align-items: center;
    }}

    .vapor-panel,
    .vapor-subtle-panel {{
      position: relative;
      border: 1px solid rgba(39, 48, 74, 0.1);
      backdrop-filter: blur(12px) saturate(120%);
    }}

    @media (prefers-color-scheme: dark) {{
      .vapor-panel,
      .vapor-subtle-panel {{
        border-color: rgba(238, 242, 255, 0.1);
      }}
    }}

    .vapor-panel {{
      width: 100%;
      border-radius: 24px;
      background: var(--panel-bg);
      box-shadow: var(--panel-shadow);
      padding: 28px;
    }}

    .vapor-subtle-panel {{
      background: var(--subtle-bg);
      box-shadow: var(--subtle-shadow);
      border-radius: 18px;
    }}

    .hero {{
      display: flex;
      flex-direction: column;
      gap: 24px;
    }}

    .hero-top {{
      display: flex;
      flex-wrap: wrap;
      justify-content: space-between;
      gap: 16px;
      align-items: flex-start;
    }}

    .brand {{
      display: flex;
      flex-direction: column;
      gap: 6px;
      min-width: 0;
    }}

    .brand-title {{
      margin: 0;
      font-size: 1.125rem;
      font-weight: 600;
      letter-spacing: -0.02em;
    }}

    .brand-copy {{
      margin: 0;
      color: var(--soft);
      font-size: 0.95rem;
    }}

    .badge-row {{
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      align-items: center;
    }}

    .vapor-badge {{
      display: inline-flex;
      align-items: center;
      border-radius: 999px;
      border: 1px solid rgba(39, 48, 74, 0.1);
      background: rgba(255, 255, 255, 0.78);
      box-shadow: 0 6px 16px rgba(111, 108, 246, 0.04);
      padding: 7px 12px;
      font-size: 11px;
      font-weight: 600;
      letter-spacing: 0.14em;
      text-transform: uppercase;
      color: var(--muted);
    }}

    @media (prefers-color-scheme: dark) {{
      .vapor-badge {{
        border-color: rgba(238, 242, 255, 0.1);
        background: rgba(22, 27, 46, 0.78);
        box-shadow: 0 6px 16px rgba(143, 140, 255, 0.04);
      }}
    }}

    .vapor-badge.error {{
      color: var(--error);
      border-color: color-mix(in srgb, var(--error) 24%, transparent);
      background: color-mix(in srgb, var(--error) 10%, var(--base-100));
    }}

    .headline {{
      max-width: 44rem;
    }}

    h1 {{
      margin: 0;
      font-size: clamp(2rem, 4vw, 3.2rem);
      line-height: 1.02;
      letter-spacing: -0.04em;
      font-weight: 600;
    }}

    .summary {{
      margin: 14px 0 0;
      max-width: 42rem;
      color: var(--muted);
      font-size: 1rem;
      line-height: 1.7;
    }}

    .notice {{
      padding: 16px 18px;
      border: 1px solid color-mix(in srgb, var(--error) 18%, transparent);
      background: color-mix(in srgb, var(--error) 8%, var(--base-100));
    }}

    .notice strong {{
      display: block;
      margin-bottom: 6px;
      font-size: 0.95rem;
    }}

    .notice p {{
      margin: 0;
      color: var(--muted);
    }}

    .meta-grid {{
      display: grid;
      gap: 14px;
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }}

    .meta-card {{
      padding: 18px;
    }}

    .detail-card {{
      grid-column: 1 / -1;
    }}

    .meta-label {{
      margin: 0 0 10px;
      color: var(--soft);
      font-size: 11px;
      font-weight: 600;
      letter-spacing: 0.14em;
      text-transform: uppercase;
    }}

    .meta-value {{
      margin: 0;
      font-size: 1rem;
      font-weight: 600;
      letter-spacing: -0.02em;
      color: var(--base-content);
      overflow-wrap: anywhere;
    }}

    .meta-copy {{
      margin: 10px 0 0;
      color: var(--muted);
      font-size: 0.95rem;
      line-height: 1.65;
      overflow-wrap: anywhere;
    }}

    .meta-copy.mono {{
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
      font-size: 0.93rem;
    }}

    .actions {{
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      justify-content: flex-start;
    }}

    .vapor-button-primary,
    .vapor-button-secondary {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-height: 48px;
      padding: 0 18px;
      border-radius: 16px;
      text-decoration: none;
      font-size: 0.95rem;
      font-weight: 600;
      font-family: inherit;
      cursor: pointer;
      transition: transform 0.2s ease, filter 0.2s ease;
    }}

    .vapor-button-primary {{
      border: 1px solid color-mix(in srgb, var(--primary) 34%, transparent);
      background: linear-gradient(135deg, var(--primary), var(--secondary));
      box-shadow: var(--button-shadow);
      color: #fff;
    }}

    .vapor-button-secondary {{
      border: 1px solid rgba(39, 48, 74, 0.1);
      background: rgba(255, 255, 255, 0.78);
      color: var(--base-content);
      box-shadow: 0 6px 16px rgba(111, 108, 246, 0.04);
    }}

    @media (prefers-color-scheme: dark) {{
      .vapor-button-primary {{
        color: #14182c;
      }}

      .vapor-button-secondary {{
        border-color: rgba(238, 242, 255, 0.1);
        background: rgba(22, 27, 46, 0.78);
        box-shadow: 0 6px 16px rgba(143, 140, 255, 0.04);
      }}
    }}

    .vapor-button-primary:hover,
    .vapor-button-secondary:hover {{
      transform: translateY(-1px);
      filter: saturate(1.03);
    }}

    .vapor-button-primary:focus-visible,
    .vapor-button-secondary:focus-visible {{
      outline: 2px solid color-mix(in srgb, var(--accent) 82%, transparent);
      outline-offset: 3px;
    }}

    @media (max-width: 720px) {{
      .vapor-panel {{
        padding: 22px;
        border-radius: 22px;
      }}

      .meta-grid {{
        grid-template-columns: 1fr;
      }}
    }}
  </style>
</head>
<body>
  <div class="vapor-shell">
    <main class="vapor-frame">
      <section class="vapor-panel hero">
        <div class="hero-top">
          <div class="brand">
            <p class="brand-title">NeoMist</p>
            <p class="brand-copy">Local-first .eth and .wei browsing</p>
          </div>

          <div class="badge-row">
            <span class="vapor-badge error">NeoMist error page</span>
            <span class="vapor-badge">{status_text}</span>
          </div>
        </div>

        <div class="headline">
          <h1>{title}</h1>
          <p class="summary">{summary}</p>
        </div>

        <section class="vapor-subtle-panel notice">
          <strong>Requested site did not load.</strong>
          <p>NeoMist generated this page because content could not be loaded from resolver or local Kubo gateway. This is not actual website response.</p>
        </section>

        <section class="meta-grid">
          <article class="vapor-subtle-panel meta-card">
            <p class="meta-label">Requested URL</p>
            <p class="meta-copy mono">{request_target}</p>
          </article>

          <article class="vapor-subtle-panel meta-card">
            <p class="meta-label">Status</p>
            <p class="meta-value">{status_text}</p>
            <p class="meta-copy">Error happened before requested site content could render.</p>
          </article>

          {detail_block}
        </section>

        <div class="actions">
          <button class="vapor-button-secondary" type="button" onclick="window.location.reload()">Retry this page</button>
          <a class="vapor-button-primary" href="https://neomist.localhost">Open NeoMist dashboard</a>
        </div>
      </section>
    </main>
  </div>
</body>
</html>"#,
        title = escape_html(title),
        summary = escape_html(summary),
        status_text = escape_html(&status_text),
        request_target = escape_html(&request_target),
        detail_block = detail_block,
    );

    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

fn build_text_error_response(
    status: StatusCode,
    title: &str,
    summary: &str,
    detail: Option<&str>,
    host: &str,
    path: &str,
) -> Response<Body> {
    let path = if path.is_empty() { "/" } else { path };
    let mut body = format!("{title}\n{summary}\nHost: {host}\nPath: {path}");
    if let Some(detail) = detail.filter(|value| !value.trim().is_empty()) {
        body.push_str("\nDetail: ");
        body.push_str(detail.trim());
    }

    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn looks_like_html_error(content_type: Option<&str>, body: &str) -> bool {
    if content_type
        .map(|value| value.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
    {
        return true;
    }

    let trimmed = body.trim_start().to_ascii_lowercase();
    trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html")
}

fn extract_html_title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let title_open_end = lower[start..].find('>')? + start + 1;
    let title_close = lower[title_open_end..].find("</title>")? + title_open_end;
    let title = decode_html_entities(&strip_html_tags(&body[title_open_end..title_close]));
    let title = collapse_whitespace(&title);
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn extract_gateway_path_error(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("failed to resolve ")?;
    Some(body[start..].trim().to_string())
}

fn decode_html_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start + 1..];

        let Some(end) = rest.find(';') else {
            decoded.push('&');
            decoded.push_str(rest);
            return decoded;
        };

        let entity = &rest[..end];
        if let Some(ch) = decode_html_entity(entity) {
            decoded.push(ch);
        } else {
            decoded.push('&');
            decoded.push_str(entity);
            decoded.push(';');
        }

        rest = &rest[end + 1..];
    }

    decoded.push_str(rest);
    decoded
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            if let Some(hex) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(decimal) = entity.strip_prefix('#') {
                decimal.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                None
            }
        }
    }
}

fn strip_html_tags(body: &str) -> String {
    let mut stripped = String::with_capacity(body.len());
    let mut in_tag = false;

    for ch in body.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }

    stripped
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_text(value: &str) -> String {
    const MAX_CHARS: usize = 220;
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_site_error_response, error_body_looks_like_html, prefers_html_error_page,
        summarize_error_detail,
    };
    use axum::body::to_bytes;
    use axum::http::{
        HeaderMap, HeaderValue, StatusCode,
        header::CONTENT_TYPE,
    };

    #[test]
    fn prefers_html_when_fetch_dest_is_document() {
        let mut headers = HeaderMap::new();
        headers.insert("sec-fetch-dest", HeaderValue::from_static("document"));

        assert!(prefers_html_error_page(&headers));
    }

    #[test]
    fn prefers_html_when_accept_header_contains_html() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
        );

        assert!(prefers_html_error_page(&headers));
    }

    #[test]
    fn summarizes_error_body() {
        let summary = summarize_error_detail(None, b"upstream\nerror\nbody").unwrap();

        assert_eq!(summary, "upstream error body");
    }

    #[test]
    fn summarizes_html_error_body_without_raw_markup() {
        let html = b"<!doctype html><html><head><title>404 page not found</title></head><body><h1>missing path</h1></body></html>";
        let summary = summarize_error_detail(Some("text/html; charset=utf-8"), html).unwrap();

        assert_eq!(summary, "404 page not found");
    }

    #[test]
    fn prefers_gateway_path_error_over_html_title() {
        let html = b"<!doctype html><html><head><title>404 page not found</title></head><body><pre>failed to resolve /ipfs/root/profile/neomist.eth: no link named &#34;neomist.eth&#34; under bafyroot</pre></body></html>";
        let summary = summarize_error_detail(Some("text/html; charset=utf-8"), html).unwrap();

        assert_eq!(
            summary,
            "failed to resolve /ipfs/root/profile/neomist.eth: no link named \"neomist.eth\" under bafyroot"
        );
    }

    #[test]
    fn detects_html_error_body() {
        let html = b"<!doctype html><html><body>broken</body></html>";

        assert!(error_body_looks_like_html(
            Some("text/html; charset=utf-8"),
            html
        ));
    }

    #[tokio::test]
    async fn builds_html_response_when_requested() {
        let response = build_site_error_response(
            StatusCode::BAD_GATEWAY,
            true,
            "Content load failed",
            "NeoMist could not load content from local Kubo gateway.",
            Some("gateway timeout"),
            "example.eth",
            "/",
        );

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("NeoMist error page"));
        assert!(body.contains("example.eth"));
        assert!(body.contains("gateway timeout"));
        assert!(body.contains("Retry this page"));
        assert!(body.contains("Open NeoMist dashboard"));
    }
}
