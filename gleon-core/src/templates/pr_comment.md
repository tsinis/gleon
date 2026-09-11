### ❌ Gleon Visual Regression Failure ({{ total_failed }} diffs)

{% if has_image_urls -%}
| Test Name | Expected | Actual | Diff | Delta |
| :--- | :---: | :---: | :---: | :---: |
{% for row in rows -%}
| `{{ row.name }}` | {{ row.expected }} | {{ row.actual }} | {{ row.diff }} | `{{ row.delta }}` |
{% endfor -%}
{%- else -%}
| Test Name | Status | Error |
| :--- | :--- | :--- |
{% for row in rows -%}
| `{{ row.name }}` | {{ row.status }} | {{ row.error }} |
{% endfor -%}
{%- endif %}
{% if remaining > 0 %}
> ⚠️ **Truncated {{ remaining }} additional diffs.** {% if html_artifact_url -%}
Download the full [Gleon HTML Report]({{ html_artifact_url }}) to inspect.
{%- else -%}
Download the full HTML Report from GitHub Action Artifacts to inspect.
{%- endif %}
{% endif -%}
{{ footer }}
