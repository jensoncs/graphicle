// Build tags a module's own build passes to the go command. A module whose
// generated code is selected by a tag (vtprotobuf, for one) fails type checking
// without it, and every importer is then incomplete: its calls go unresolved.
package main

import (
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

const maxInferredTags = 16
const maxBuildFileBytes = 1 << 20

var (
	goCommand = regexp.MustCompile(`\bgo\s+(?:build|test|vet|install|run|list)\b|\bGOFLAGS\b`)
	tagsFlag  = regexp.MustCompile(`-tags[= ](?:'([^']*)'|"([^"]*)"|([^\s"']+))`)
)

// moduleBuildTags returns the profile's tags when it names any. Otherwise it
// returns the tags of go commands in the module directory's own build files
// (makefiles, Dockerfiles, Jenkinsfiles, shell scripts; not subdirectories, not
// YAML, whose content the analysis cache key leaves out), sorted and bounded.
// Comment lines do not count: a commented-out command is not the build.
func moduleBuildTags(dir string, explicit []string) []string {
	if len(explicit) > 0 {
		return explicit
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	found := map[string]bool{}
	for _, entry := range entries {
		if !entry.Type().IsRegular() || !isBuildFile(entry.Name()) {
			continue
		}
		info, err := entry.Info()
		if err != nil || info.Size() > maxBuildFileBytes {
			continue
		}
		data, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(data), "\n") {
			trimmed := strings.TrimSpace(line)
			if strings.HasPrefix(trimmed, "#") || strings.HasPrefix(trimmed, "//") || !goCommand.MatchString(line) {
				continue
			}
			for _, match := range tagsFlag.FindAllStringSubmatch(line, -1) {
				for _, tag := range strings.FieldsFunc(match[1]+match[2]+match[3], func(r rune) bool { return r == ',' || r == ' ' }) {
					if validBuildTag(tag) {
						found[tag] = true
					}
				}
			}
		}
	}
	tags := make([]string, 0, len(found))
	for tag := range found {
		tags = append(tags, tag)
	}
	sort.Strings(tags)
	if len(tags) > maxInferredTags {
		tags = tags[:maxInferredTags]
	}
	return tags
}

func isBuildFile(name string) bool {
	switch {
	case name == "Makefile" || name == "makefile" || name == "GNUmakefile" || name == "Jenkinsfile" || name == "Dockerfile":
		return true
	case strings.HasPrefix(name, "Dockerfile.") || strings.HasSuffix(name, ".Dockerfile"):
		return true
	case strings.HasSuffix(name, ".mk") || strings.HasSuffix(name, ".sh"):
		return true
	}
	return false
}

// The same shape the review profile accepts: 1 to 100 of [A-Za-z0-9_.].
func validBuildTag(tag string) bool {
	if tag == "" || len(tag) > 100 {
		return false
	}
	for _, b := range []byte(tag) {
		if !(b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9' || b == '_' || b == '.') {
			return false
		}
	}
	return true
}
