package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	maxExchangeBytes = 96 << 20
	maxRevision      = 9_007_199_254_740_991
	maxReplayEntries = 1024
	contentType      = "application/vnd.norishell.ssh-sync-exchange+json;version=1"
)

type replayEntry struct {
	Key       string `json:"key"`
	Method    string `json:"method"`
	Digest    string `json:"digest"`
	Condition string `json:"condition"`
	Status    int    `json:"status"`
	Revision  uint64 `json:"revision,omitempty"`
	ETag      string `json:"etag,omitempty"`
	UpdatedMS int64  `json:"updated_ms,omitempty"`
}

type diskState struct {
	Version      int           `json:"version"`
	NextRevision uint64        `json:"next_revision"`
	Revision     uint64        `json:"revision,omitempty"`
	ETag         string        `json:"etag,omitempty"`
	UpdatedMS    int64         `json:"updated_ms,omitempty"`
	Body         []byte        `json:"body,omitempty"`
	Replay       []replayEntry `json:"replay,omitempty"`
}

type exchangeStore struct {
	mu       sync.Mutex
	path     string
	state    diskState
	poisoned bool
}

func openExchangeStore(path string) (*exchangeStore, error) {
	s := &exchangeStore{path: path, state: diskState{Version: 1, NextRevision: 1}}
	bytes, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return s, nil
	}
	if err != nil {
		return nil, err
	}
	if err := json.Unmarshal(bytes, &s.state); err != nil {
		return nil, fmt.Errorf("invalid exchange state: %w", err)
	}
	if s.state.Version != 1 || s.state.NextRevision == 0 || s.state.NextRevision > maxRevision || len(s.state.Body) > maxExchangeBytes ||
		(s.state.Revision == 0) != (s.state.ETag == "") ||
		(s.state.Revision != 0 && (s.state.Revision >= s.state.NextRevision || s.state.ETag != strongETag(s.state.Revision, s.state.Body))) {
		return nil, errors.New("invalid exchange state")
	}
	return s, nil
}

func strongETag(revision uint64, body []byte) string {
	hash := sha256.Sum256(body)
	return fmt.Sprintf("\"r%d-%x\"", revision, hash)
}

func digestHex(body []byte) string {
	hash := sha256.Sum256(body)
	return hex.EncodeToString(hash[:])
}

func (s *exchangeStore) get() (diskState, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state, s.poisoned
}

func (s *exchangeStore) mutate(method, key, digest, condition string, body []byte) (replayEntry, int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.poisoned {
		return replayEntry{}, 503
	}
	for _, prior := range s.state.Replay {
		if prior.Key == key {
			if prior.Method == method && prior.Digest == digest && prior.Condition == condition {
				return prior, prior.Status
			}
			return replayEntry{}, 409
		}
	}
	if s.state.NextRevision >= maxRevision {
		return replayEntry{}, 507
	}
	if method == "PUT" {
		if s.state.Revision == 0 {
			if condition != "next:"+strconv.FormatUint(s.state.NextRevision, 10) {
				return replayEntry{}, 412
			}
		} else if condition != "etag:"+s.state.ETag {
			return replayEntry{}, 412
		}
	} else if s.state.Revision == 0 {
		return replayEntry{}, 404
	} else if condition != "etag:"+s.state.ETag {
		return replayEntry{}, 412
	}
	next := s.state
	next.Replay = append([]replayEntry(nil), next.Replay...)
	entry := replayEntry{Key: key, Method: method, Digest: digest, Condition: condition}
	if method == "PUT" {
		entry.Status = 200
		if next.Revision == 0 {
			entry.Status = 201
		}
		entry.Revision = next.NextRevision
		entry.ETag = strongETag(entry.Revision, body)
		entry.UpdatedMS = time.Now().UnixMilli()
		next.Revision = entry.Revision
		next.ETag = entry.ETag
		next.UpdatedMS = entry.UpdatedMS
		next.Body = body
	} else {
		entry.Status = 204
		next.Revision = 0
		next.ETag = ""
		next.UpdatedMS = 0
		next.Body = nil
	}
	next.NextRevision++
	next.Replay = append(next.Replay, entry)
	if len(next.Replay) > maxReplayEntries {
		next.Replay = next.Replay[len(next.Replay)-maxReplayEntries:]
	}
	encoded, err := json.Marshal(next)
	if err != nil {
		return replayEntry{}, 503
	}
	committed, err := writeAtomic(s.path, encoded)
	if err != nil {
		if committed {
			// A rename followed by failed directory sync has uncertain durability.
			s.poisoned = true
		}
		return replayEntry{}, 503
	}
	s.state = next
	return entry, entry.Status
}

// writeAtomic reports whether rename happened, so callers can fail closed after an uncertain commit.
func writeAtomic(path string, data []byte) (bool, error) {
	dir := filepath.Dir(path)
	f, err := os.CreateTemp(dir, ".norishell-*")
	if err != nil {
		return false, err
	}
	tmp := f.Name()
	defer os.Remove(tmp)
	if err := f.Chmod(0600); err != nil {
		f.Close()
		return false, err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return false, err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return false, err
	}
	if err := f.Close(); err != nil {
		return false, err
	}
	return replaceAndSync(tmp, path, dir)
}

func boundedBody(reader io.Reader, limit int64) ([]byte, error) {
	bytes, err := io.ReadAll(io.LimitReader(reader, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(bytes)) > limit {
		return nil, errors.New("body too large")
	}
	return bytes, nil
}

func validUUID(value string) bool {
	if len(value) != 36 {
		return false
	}
	for i, ch := range value {
		if i == 8 || i == 13 || i == 18 || i == 23 {
			if ch != '-' {
				return false
			}
		} else if !strings.ContainsRune("0123456789abcdefABCDEF", ch) {
			return false
		}
	}
	return true
}
