package main

import "C"

import (
	"bufio"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/bmatcuk/doublestar/v4"
	sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

const (
	defaultSourceDir = "~/.claude/projects"
	defaultPatterns  = "*.jsonl,**/*.jsonl"
	batchSize        = 1000
)

var outputSchema = map[string]string{
	"agent_id":                   "Utf8",
	"content_text":               "Utf8",
	"content_types":              "Utf8",
	"cwd":                        "Utf8",
	"data_json":                  "Utf8",
	"entrypoint":                 "Utf8",
	"event_subtype":              "Utf8",
	"event_type":                 "Utf8",
	"file_is_subagent":           "Boolean",
	"git_branch":                 "Utf8",
	"is_sidechain":               "Boolean",
	"line_number":                "Int64",
	"message_id":                 "Utf8",
	"message_model":              "Utf8",
	"message_role":               "Utf8",
	"message_stop_reason":        "Utf8",
	"message_type":               "Utf8",
	"message_usage_cache_creation_ephemeral_1h_input_tokens":            "Int64",
	"message_usage_cache_creation_ephemeral_5m_input_tokens":            "Int64",
	"message_usage_cache_creation_input_tokens":                         "Int64",
	"message_usage_cache_read_input_tokens":                             "Int64",
	"message_usage_inference_geo":                                       "Utf8",
	"message_usage_input_tokens":                                        "Int64",
	"message_usage_iterations_cache_creation_ephemeral_1h_input_tokens": "Int64",
	"message_usage_iterations_cache_creation_ephemeral_5m_input_tokens": "Int64",
	"message_usage_iterations_cache_creation_input_tokens":              "Int64",
	"message_usage_iterations_cache_read_input_tokens":                  "Int64",
	"message_usage_iterations_count":                                    "Int64",
	"message_usage_iterations_input_tokens":                             "Int64",
	"message_usage_iterations_output_tokens":                            "Int64",
	"message_usage_iterations_types":                                    "Utf8",
	"message_usage_output_tokens":                                       "Int64",
	"message_usage_server_tool_use_web_fetch_requests":                  "Int64",
	"message_usage_server_tool_use_web_search_requests":                 "Int64",
	"message_usage_service_tier":                                        "Utf8",
	"message_usage_speed":                                               "Utf8",
	"parent_tool_use_id":         "Utf8",
	"parent_uuid":                "Utf8",
	"permission_mode":            "Utf8",
	"project_id":                 "Utf8",
	"prompt_id":                  "Utf8",
	"request_id":                 "Utf8",
	"search_text":                "Utf8",
	"session_id":                 "Utf8",
	"slug":                       "Utf8",
	"snapshot_json":              "Utf8",
	"source_file":                "Utf8",
	"source_tool_assistant_uuid": "Utf8",
	"thinking_text":              "Utf8",
	"timestamp":                  "Timestamp",
	"tool_call_ids":              "Utf8",
	"tool_input_json":            "Utf8",
	"tool_names":                 "Utf8",
	"tool_result_error":          "Boolean",
	"tool_result_text":           "Utf8",
	"tool_use_id":                "Utf8",
	"user_type":                  "Utf8",
	"uuid":                       "Utf8",
	"version":                    "Utf8",
}

type ClaudeHistoryConnector struct{}

func init() {
	sdk.ExportConnector(&ClaudeHistoryConnector{})
}

func main() {}

func (c *ClaudeHistoryConnector) Discover(_ []string, args map[string]string) ([]sdk.Location, error) {
	sourceDir, err := resolveSourceDir(args)
	if err != nil {
		return nil, err
	}

	patterns := parsePatterns(args)
	projectFiles, err := collectProjectFiles(sourceDir, patterns)
	if err != nil {
		return nil, err
	}

	projects := make([]string, 0, len(projectFiles))
	for projectID := range projectFiles {
		projects = append(projects, projectID)
	}
	sort.Strings(projects)

	locations := make([]sdk.Location, 0, len(projects))
	for _, projectID := range projects {
		locations = append(locations, sdk.Location{
			Location: projectID,
			MustCopy: true,
			Format:   "parquet",
			Version:  projectVersion(sourceDir, projectFiles[projectID]),
		})
	}
	sort.Slice(locations, func(i, j int) bool {
		return locations[i].Location < locations[j].Location
	})
	return locations, nil
}

func (c *ClaudeHistoryConnector) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
	sourceDir, err := resolveSourceDir(args)
	if err != nil {
		return nil, err
	}

	projectFiles, err := collectProjectFiles(sourceDir, parsePatterns(args))
	if err != nil {
		return nil, err
	}

	files, ok := projectFiles[location.Location]
	if !ok {
		return nil, fmt.Errorf("project batch not found: %s", location.Location)
	}

	rows := make([]map[string]interface{}, 0, batchSize)
	records := make([]arrow.Record, 0)

	for _, relPath := range files {
		path := filepath.Join(sourceDir, filepath.FromSlash(relPath))
		file, err := os.Open(path)
		if err != nil {
			return nil, err
		}
		scanner := bufio.NewScanner(file)
		scanner.Buffer(make([]byte, 1024*1024), 20*1024*1024)
		var lineNumber int64

		for scanner.Scan() {
			lineNumber++
			line := strings.TrimSpace(scanner.Text())
			if line == "" {
				continue
			}

			var event map[string]interface{}
			if err := json.Unmarshal([]byte(line), &event); err != nil {
				file.Close()
				return nil, fmt.Errorf("parse %s line %d: %w", relPath, lineNumber, err)
			}

			rows = append(rows, buildRow(relPath, lineNumber, event))
			if len(rows) >= batchSize {
				batchRecords, err := sdk.NormalizeToRecords(rows, outputSchema)
				if err != nil {
					file.Close()
					return nil, err
				}
				records = append(records, batchRecords...)
				rows = rows[:0]
			}
		}

		if err := scanner.Err(); err != nil {
			file.Close()
			return nil, err
		}
		file.Close()
	}

	if len(rows) > 0 {
		batchRecords, err := sdk.NormalizeToRecords(rows, outputSchema)
		if err != nil {
			return nil, err
		}
		records = append(records, batchRecords...)
	}

	return records, nil
}

func buildRow(sourceFile string, lineNumber int64, event map[string]interface{}) map[string]interface{} {
	projectID := projectIDForSourceFile(sourceFile)

	row := map[string]interface{}{
		"event_subtype":              stringValue(event["subtype"]),
		"event_type":                 stringValue(event["type"]),
		"file_is_subagent":           strings.Contains(sourceFile, "/subagents/"),
		"line_number":                lineNumber,
		"parent_tool_use_id":         stringValue(event["parentToolUseID"]),
		"parent_uuid":                stringValue(event["parentUuid"]),
		"permission_mode":            stringValue(event["permissionMode"]),
		"project_id":                 projectID,
		"prompt_id":                  stringValue(event["promptId"]),
		"request_id":                 stringValue(event["requestId"]),
		"session_id":                 stringValue(event["sessionId"]),
		"slug":                       stringValue(event["slug"]),
		"source_file":                sourceFile,
		"source_tool_assistant_uuid": stringValue(event["sourceToolAssistantUUID"]),
		"timestamp":                  parseTimestampMicros(stringValue(event["timestamp"])),
		"tool_use_id":                stringValue(event["toolUseID"]),
		"user_type":                  stringValue(event["userType"]),
		"uuid":                       stringValue(event["uuid"]),
		"version":                    stringValue(event["version"]),
	}

	if value, ok := boolValue(event["isSidechain"]); ok {
		row["is_sidechain"] = value
	}
	if value := stringValue(event["cwd"]); value != "" {
		row["cwd"] = value
	}
	if value := stringValue(event["gitBranch"]); value != "" {
		row["git_branch"] = value
	}
	if value := stringValue(event["entrypoint"]); value != "" {
		row["entrypoint"] = value
	}
	if value := jsonString(event["data"]); value != "" {
		row["data_json"] = value
	}
	if value := jsonString(event["snapshot"]); value != "" {
		row["snapshot_json"] = value
	}
	if value := stringOrStructuredText(event["content"]); value != "" {
		row["content_text"] = value
	}

	textParts := []string{stringValue(row["content_text"])}
	thinkingParts := []string{}
	contentTypes := []string{}
	toolNames := []string{}
	toolCallIDs := []string{}
	toolInputs := []interface{}{}
	toolResultParts := []string{}
	toolResultUseIDs := []string{}
	var toolResultError *bool

	if message, ok := mapValue(event["message"]); ok {
		row["message_id"] = stringValue(message["id"])
		row["message_model"] = stringValue(message["model"])
		row["message_role"] = stringValue(message["role"])
		row["message_stop_reason"] = stringValue(message["stop_reason"])
		row["message_type"] = stringValue(message["type"])
		if usage, ok := mapValue(message["usage"]); ok {
			addUsageFields(row, usage)
		}

		switch content := message["content"].(type) {
		case string:
			appendIfPresent(&textParts, content)
			appendIfPresent(&contentTypes, "text")
		case []interface{}:
			for _, item := range content {
				itemMap, ok := mapValue(item)
				if !ok {
					appendIfPresent(&textParts, stringOrStructuredText(item))
					continue
				}

				itemType := stringValue(itemMap["type"])
				appendIfPresent(&contentTypes, itemType)

				switch itemType {
				case "text":
					appendIfPresent(&textParts, stringValue(itemMap["text"]))
				case "thinking":
					appendIfPresent(&thinkingParts, firstNonEmpty(
						stringValue(itemMap["thinking"]),
						stringValue(itemMap["text"]),
					))
				case "tool_use":
					appendIfPresent(&toolNames, stringValue(itemMap["name"]))
					appendIfPresent(&toolCallIDs, stringValue(itemMap["id"]))
					if input := itemMap["input"]; input != nil {
						toolInputs = append(toolInputs, input)
					}
				case "tool_result":
					appendIfPresent(&toolResultParts, stringOrStructuredText(itemMap["content"]))
					appendIfPresent(&toolResultUseIDs, stringValue(itemMap["tool_use_id"]))
					if value, ok := boolValue(itemMap["is_error"]); ok {
						if toolResultError == nil {
							toolResultError = new(bool)
						}
						*toolResultError = *toolResultError || value
					}
				default:
					appendIfPresent(&textParts, fallbackText(itemMap))
				}
			}
		default:
			appendIfPresent(&textParts, stringOrStructuredText(content))
		}
	}

	row["content_text"] = joined(textParts)
	row["thinking_text"] = joined(thinkingParts)
	row["content_types"] = joinedWithComma(contentTypes)
	row["tool_names"] = joinedWithComma(toolNames)
	row["tool_call_ids"] = joinedWithComma(toolCallIDs)
	row["tool_result_text"] = joined(toolResultParts)
	if len(toolInputs) > 0 {
		row["tool_input_json"] = jsonString(toolInputs)
	}
	if toolResultError != nil {
		row["tool_result_error"] = *toolResultError
	}
	// Claude Code nests `tool_use_id` inside each `tool_result` content block;
	// the top-level `event["toolUseID"]` only appears on a few sub-agent
	// metadata events. Prefer the nested ID(s) so result rows can be joined
	// back to the assistant's tool_use via the same id.
	if len(toolResultUseIDs) > 0 {
		row["tool_use_id"] = joinedWithComma(toolResultUseIDs)
	}

	searchParts := []string{
		stringValue(row["content_text"]),
		stringValue(row["thinking_text"]),
		stringValue(row["tool_result_text"]),
		strings.Join(toolNames, " "),
		stringValue(row["event_type"]),
		stringValue(row["message_role"]),
		stringValue(row["message_model"]),
	}
	row["search_text"] = joined(searchParts)

	if value := stringValue(event["agentId"]); value != "" {
		row["agent_id"] = value
	}

	return row
}

func collectProjectFiles(sourceDir string, patterns []string) (map[string][]string, error) {
	projectFiles := make(map[string][]string)

	err := filepath.WalkDir(sourceDir, func(path string, d os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if d.IsDir() {
			return nil
		}

		relPath, err := filepath.Rel(sourceDir, path)
		if err != nil {
			return err
		}
		relPath = filepath.ToSlash(relPath)
		if !matchesPatterns(relPath, patterns) {
			return nil
		}

		projectID := projectIDForSourceFile(relPath)
		projectFiles[projectID] = append(projectFiles[projectID], relPath)
		return nil
	})
	if err != nil {
		return nil, err
	}

	for projectID := range projectFiles {
		sort.Strings(projectFiles[projectID])
	}
	return projectFiles, nil
}

func projectVersion(sourceDir string, files []string) string {
	hasher := fnv.New64a()
	for _, relPath := range files {
		_, _ = hasher.Write([]byte(relPath))
		_, _ = hasher.Write([]byte{0})
		fullPath := filepath.Join(sourceDir, filepath.FromSlash(relPath))
		if info, err := os.Stat(fullPath); err == nil {
			_, _ = hasher.Write([]byte(fileVersion(info)))
		}
		_, _ = hasher.Write([]byte{'\n'})
	}
	return fmt.Sprintf("%d-%x", len(files), hasher.Sum64())
}

func projectIDForSourceFile(sourceFile string) string {
	projectID := sourceFile
	if slash := strings.IndexByte(sourceFile, '/'); slash >= 0 {
		projectID = sourceFile[:slash]
	}
	return projectID
}

func resolveSourceDir(args map[string]string) (string, error) {
	candidate := firstNonEmpty(args["source_dir"], args["url"], defaultSourceDir)
	if strings.HasPrefix(candidate, "file://") {
		candidate = strings.TrimPrefix(candidate, "file://")
	}
	if strings.HasPrefix(candidate, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		candidate = filepath.Join(home, candidate[2:])
	}
	path, err := filepath.Abs(candidate)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(path)
	if err != nil {
		return "", err
	}
	if !info.IsDir() {
		return "", fmt.Errorf("source directory is not a directory: %s", path)
	}
	return path, nil
}

func parsePatterns(args map[string]string) []string {
	patterns := firstNonEmpty(args["patterns"], defaultPatterns)
	parts := strings.Split(patterns, ",")
	normalized := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			normalized = append(normalized, part)
		}
	}
	return normalized
}

func matchesPatterns(path string, patterns []string) bool {
	for _, pattern := range patterns {
		matched, err := doublestar.Match(pattern, path)
		if err == nil && matched {
			return true
		}
	}
	return false
}

func fileVersion(info os.FileInfo) string {
	return fmt.Sprintf("%d-%d", info.Size(), info.ModTime().UnixNano())
}

func mapValue(value interface{}) (map[string]interface{}, bool) {
	asMap, ok := value.(map[string]interface{})
	return asMap, ok
}

func boolValue(value interface{}) (bool, bool) {
	asBool, ok := value.(bool)
	return asBool, ok
}

func int64Value(value interface{}) (int64, bool) {
	switch typed := value.(type) {
	case int:
		return int64(typed), true
	case int64:
		return typed, true
	case float64:
		return int64(typed), true
	case json.Number:
		parsed, err := typed.Int64()
		if err == nil {
			return parsed, true
		}
		return 0, false
	default:
		return 0, false
	}
}

// parseTimestampMicros converts an ISO-8601 string (the format Claude Code
// writes for `timestamp` fields) to microseconds since the Unix epoch — the
// units the SDK's Timestamp_us builder expects. Returns nil for empty or
// unparseable inputs so the row gets a real null instead of a sentinel zero.
func parseTimestampMicros(s string) interface{} {
	if s == "" {
		return nil
	}
	t, err := time.Parse(time.RFC3339Nano, s)
	if err != nil {
		return nil
	}
	return t.UnixMicro()
}

func stringValue(value interface{}) string {
	if value == nil {
		return ""
	}
	switch typed := value.(type) {
	case string:
		return typed
	case json.Number:
		return typed.String()
	case float64:
		return fmt.Sprintf("%.0f", typed)
	case bool:
		if typed {
			return "true"
		}
		return "false"
	default:
		return ""
	}
}

func stringOrStructuredText(value interface{}) string {
	switch typed := value.(type) {
	case nil:
		return ""
	case string:
		return typed
	case []interface{}:
		parts := make([]string, 0, len(typed))
		for _, item := range typed {
			appendIfPresent(&parts, stringOrStructuredText(item))
		}
		return joined(parts)
	case map[string]interface{}:
		return fallbackText(typed)
	default:
		return fmt.Sprint(typed)
	}
}

func fallbackText(value map[string]interface{}) string {
	parts := []string{
		stringValue(value["text"]),
		stringValue(value["thinking"]),
		stringOrStructuredText(value["content"]),
	}
	if text := joined(parts); text != "" {
		return text
	}
	return jsonString(value)
}

func jsonString(value interface{}) string {
	if value == nil {
		return ""
	}
	bytes, err := json.Marshal(value)
	if err != nil {
		return ""
	}
	return string(bytes)
}

func appendIfPresent(dest *[]string, value string) {
	value = strings.TrimSpace(value)
	if value != "" {
		*dest = append(*dest, value)
	}
}

func joined(parts []string) string {
	trimmed := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			trimmed = append(trimmed, part)
		}
	}
	return strings.Join(trimmed, "\n\n")
}

func joinedWithComma(parts []string) string {
	trimmed := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			trimmed = append(trimmed, part)
		}
	}
	return strings.Join(trimmed, ", ")
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

func addUsageFields(row map[string]interface{}, usage map[string]interface{}) {
	setInt64Field(row, "message_usage_input_tokens", usage["input_tokens"])
	setInt64Field(row, "message_usage_output_tokens", usage["output_tokens"])
	setInt64Field(row, "message_usage_cache_creation_input_tokens", usage["cache_creation_input_tokens"])
	setInt64Field(row, "message_usage_cache_read_input_tokens", usage["cache_read_input_tokens"])
	setStringField(row, "message_usage_inference_geo", usage["inference_geo"])
	setStringField(row, "message_usage_service_tier", usage["service_tier"])
	setStringField(row, "message_usage_speed", usage["speed"])

	if cacheCreation, ok := mapValue(usage["cache_creation"]); ok {
		setInt64Field(row, "message_usage_cache_creation_ephemeral_1h_input_tokens", cacheCreation["ephemeral_1h_input_tokens"])
		setInt64Field(row, "message_usage_cache_creation_ephemeral_5m_input_tokens", cacheCreation["ephemeral_5m_input_tokens"])
	}

	if serverToolUse, ok := mapValue(usage["server_tool_use"]); ok {
		setInt64Field(row, "message_usage_server_tool_use_web_fetch_requests", serverToolUse["web_fetch_requests"])
		setInt64Field(row, "message_usage_server_tool_use_web_search_requests", serverToolUse["web_search_requests"])
	}

	iterations, ok := usage["iterations"].([]interface{})
	if !ok || len(iterations) == 0 {
		return
	}

	row["message_usage_iterations_count"] = int64(len(iterations))
	iterationTypes := make([]string, 0, len(iterations))
	var inputTokens int64
	var outputTokens int64
	var cacheCreationInputTokens int64
	var cacheReadInputTokens int64
	var ephemeral1hInputTokens int64
	var ephemeral5mInputTokens int64

	for _, iteration := range iterations {
		iterationMap, ok := mapValue(iteration)
		if !ok {
			continue
		}

		appendIfPresent(&iterationTypes, stringValue(iterationMap["type"]))
		inputTokens += int64OrZero(iterationMap["input_tokens"])
		outputTokens += int64OrZero(iterationMap["output_tokens"])
		cacheCreationInputTokens += int64OrZero(iterationMap["cache_creation_input_tokens"])
		cacheReadInputTokens += int64OrZero(iterationMap["cache_read_input_tokens"])

		if cacheCreation, ok := mapValue(iterationMap["cache_creation"]); ok {
			ephemeral1hInputTokens += int64OrZero(cacheCreation["ephemeral_1h_input_tokens"])
			ephemeral5mInputTokens += int64OrZero(cacheCreation["ephemeral_5m_input_tokens"])
		}
	}

	row["message_usage_iterations_types"] = joinedWithComma(iterationTypes)
	row["message_usage_iterations_input_tokens"] = inputTokens
	row["message_usage_iterations_output_tokens"] = outputTokens
	row["message_usage_iterations_cache_creation_input_tokens"] = cacheCreationInputTokens
	row["message_usage_iterations_cache_read_input_tokens"] = cacheReadInputTokens
	row["message_usage_iterations_cache_creation_ephemeral_1h_input_tokens"] = ephemeral1hInputTokens
	row["message_usage_iterations_cache_creation_ephemeral_5m_input_tokens"] = ephemeral5mInputTokens
}

func setInt64Field(row map[string]interface{}, key string, value interface{}) {
	if parsed, ok := int64Value(value); ok {
		row[key] = parsed
	}
}

func setStringField(row map[string]interface{}, key string, value interface{}) {
	if parsed := stringValue(value); parsed != "" {
		row[key] = parsed
	}
}

func int64OrZero(value interface{}) int64 {
	if parsed, ok := int64Value(value); ok {
		return parsed
	}
	return 0
}
