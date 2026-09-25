package main

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/argon2"
)

const (
	accessLifetime  = 15 * time.Minute
	refreshLifetime = 7 * 24 * time.Hour
	maxSessions     = 1024
)

type passwordState struct {
	Version int    `json:"version"`
	Epoch   string `json:"epoch"`
	Salt    string `json:"salt"`
	Hash    string `json:"hash"`
}

type session struct {
	Kind      string `json:"kind"`
	Epoch     string `json:"epoch"`
	ExpiresMS int64  `json:"expires_ms"`
	Pair      string `json:"pair"`
}

type sessionState struct {
	Version int                `json:"version"`
	Tokens  map[string]session `json:"tokens"`
}

type authStore struct {
	mu               sync.Mutex
	path             string
	sessions         sessionState
	poisoned         bool
	attempts         chan struct{}
	passwordAttempts []time.Time
}

// reservePasswordAttempt limits guesses across both the API and browser login paths.
func (a *authStore) reservePasswordAttempt() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	now := time.Now()
	remaining := a.passwordAttempts[:0]
	for _, attempt := range a.passwordAttempts {
		if now.Sub(attempt) < time.Minute {
			remaining = append(remaining, attempt)
		}
	}
	a.passwordAttempts = remaining
	if len(remaining) >= 10 {
		return false
	}
	a.passwordAttempts = append(a.passwordAttempts, now)
	return true
}

func randomString(size int) (string, error) {
	bytes := make([]byte, size)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(bytes), nil
}

func setPassword(path string, input io.Reader) error {
	bytes, err := boundedBody(input, 1025)
	if err != nil {
		return err
	}
	password := strings.TrimSuffix(strings.TrimSuffix(string(bytes), "\n"), "\r")
	if len(password) < 12 || len(password) > 1024 || strings.ContainsAny(password, "\r\n\x00") {
		return errors.New("password must be 12-1024 bytes on one line")
	}
	salt := make([]byte, 16)
	if _, err := rand.Read(salt); err != nil {
		return err
	}
	epoch, err := randomString(16)
	if err != nil {
		return err
	}
	hash := argon2.IDKey([]byte(password), salt, 3, 64*1024, 4, 32)
	state := passwordState{
		Version: 1,
		Epoch:   epoch,
		Salt:    base64.RawStdEncoding.EncodeToString(salt),
		Hash:    base64.RawStdEncoding.EncodeToString(hash),
	}
	encoded, err := json.Marshal(state)
	if err != nil {
		return err
	}
	committed, err := writeAtomic(path, encoded)
	if committed && err != nil {
		return fmt.Errorf("password update may have committed; check which password works before retrying or restarting the service: %w", err)
	}
	return err
}

func readPassword(path string) (passwordState, error) {
	var state passwordState
	bytes, err := os.ReadFile(path)
	if err != nil {
		return state, err
	}
	if len(bytes) > 4096 || json.Unmarshal(bytes, &state) != nil || state.Version != 1 || state.Epoch == "" {
		return state, errors.New("invalid password state")
	}
	salt, err := base64.RawStdEncoding.DecodeString(state.Salt)
	if err != nil || len(salt) != 16 {
		return state, errors.New("invalid password salt")
	}
	hash, err := base64.RawStdEncoding.DecodeString(state.Hash)
	if err != nil || len(hash) != 32 {
		return state, errors.New("invalid password hash")
	}
	return state, nil
}

func checkPassword(state passwordState, password string) bool {
	if len(password) > 1024 {
		return false
	}
	salt, _ := base64.RawStdEncoding.DecodeString(state.Salt)
	expected, _ := base64.RawStdEncoding.DecodeString(state.Hash)
	actual := argon2.IDKey([]byte(password), salt, 3, 64*1024, 4, 32)
	return subtle.ConstantTimeCompare(actual, expected) == 1
}

func openAuthStore(dataDir string) (*authStore, error) {
	a := &authStore{
		path:     filepath.Join(dataDir, "sessions.json"),
		sessions: sessionState{Version: 1, Tokens: make(map[string]session)},
		attempts: make(chan struct{}, 2),
	}
	bytes, err := os.ReadFile(a.path)
	if errors.Is(err, os.ErrNotExist) {
		return a, nil
	}
	if err != nil {
		return nil, err
	}
	if err := json.Unmarshal(bytes, &a.sessions); err != nil || a.sessions.Version != 1 || a.sessions.Tokens == nil || len(a.sessions.Tokens) > maxSessions {
		return nil, errors.New("invalid sessions state")
	}
	return a, nil
}

func tokenHash(token string) string {
	hash := sha256.Sum256([]byte(token))
	return hex.EncodeToString(hash[:])
}

func (a *authStore) authenticate(bearer, epoch string) bool {
	if len(bearer) != 43 || strings.ContainsAny(bearer, "\r\n \t") {
		return false
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.poisoned {
		return false
	}
	s, ok := a.sessions.Tokens[tokenHash(bearer)]
	return ok && s.Kind == "access" && s.Epoch == epoch && s.ExpiresMS > time.Now().UnixMilli()
}

func (a *authStore) pair(epoch string, consumedRefresh string) (string, string, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.poisoned {
		return "", "", errors.New("session state uncertain")
	}
	now := time.Now().UnixMilli()
	priorPair := ""
	if consumedRefresh != "" {
		prior, ok := a.sessions.Tokens[tokenHash(consumedRefresh)]
		if !ok || prior.Kind != "refresh" || prior.Epoch != epoch || prior.ExpiresMS <= now {
			return "", "", errors.New("invalid refresh token")
		}
		priorPair = prior.Pair
	}
	access, err := randomString(32)
	if err != nil {
		return "", "", err
	}
	refresh, err := randomString(32)
	if err != nil {
		return "", "", err
	}
	pair, err := randomString(16)
	if err != nil {
		return "", "", err
	}
	next := sessionState{Version: 1, Tokens: make(map[string]session)}
	for hash, item := range a.sessions.Tokens {
		if item.ExpiresMS > now && item.Epoch == epoch && item.Pair != priorPair {
			next.Tokens[hash] = item
		}
	}
	next.Tokens[tokenHash(access)] = session{Kind: "access", Epoch: epoch, ExpiresMS: now + accessLifetime.Milliseconds(), Pair: pair}
	next.Tokens[tokenHash(refresh)] = session{Kind: "refresh", Epoch: epoch, ExpiresMS: now + refreshLifetime.Milliseconds(), Pair: pair}
	if len(next.Tokens) > maxSessions {
		return "", "", errors.New("too many sessions")
	}
	if err := a.persist(next); err != nil {
		return "", "", err
	}
	return access, refresh, nil
}

func (a *authStore) revoke(epoch, token string) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.poisoned {
		return errors.New("session state uncertain")
	}
	prior, ok := a.sessions.Tokens[tokenHash(token)]
	if !ok || prior.Epoch != epoch {
		return nil
	}
	next := sessionState{Version: 1, Tokens: make(map[string]session)}
	for hash, item := range a.sessions.Tokens {
		if item.Pair != prior.Pair {
			next.Tokens[hash] = item
		}
	}
	return a.persist(next)
}

func (a *authStore) persist(next sessionState) error {
	encoded, err := json.Marshal(next)
	if err != nil {
		return err
	}
	committed, err := writeAtomic(a.path, encoded)
	if err != nil {
		if committed {
			a.poisoned = true
		}
		return err
	}
	a.sessions = next
	return nil
}

func readForm(w http.ResponseWriter, r *http.Request) bool {
	if r.Header.Get("Content-Type") != "application/x-www-form-urlencoded" {
		http.Error(w, "form required", http.StatusBadRequest)
		return false
	}
	r.Body = http.MaxBytesReader(w, r.Body, 8192)
	if err := r.ParseForm(); err != nil {
		http.Error(w, "invalid form", http.StatusBadRequest)
		return false
	}
	return true
}

func writeTokenResponse(w http.ResponseWriter, access, refresh string) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"access_token":  access,
		"refresh_token": refresh,
		"token_type":    "Bearer",
		"expires_in":    int(accessLifetime.Seconds()),
		"scope":         scope,
	})
}

func (a *authStore) login(w http.ResponseWriter, r *http.Request, passwordPath string) {
	if !readForm(w, r) {
		return
	}
	if r.PostForm.Get("client_id") != clientID || r.PostForm.Get("scope") != scope || r.PostForm.Get("username") != "owner" {
		http.Error(w, "invalid credentials", http.StatusUnauthorized)
		return
	}
	select {
	case a.attempts <- struct{}{}:
		defer func() { <-a.attempts }()
	default:
		http.Error(w, "busy", http.StatusTooManyRequests)
		return
	}
	if !a.reservePasswordAttempt() {
		w.Header().Set("Retry-After", "60")
		http.Error(w, "too many login attempts", http.StatusTooManyRequests)
		return
	}
	state, err := readPassword(passwordPath)
	if err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return
	}
	if !checkPassword(state, r.PostForm.Get("password")) {
		http.Error(w, "invalid credentials", http.StatusUnauthorized)
		return
	}
	access, refresh, err := a.pair(state.Epoch, "")
	if err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return
	}
	writeTokenResponse(w, access, refresh)
}

func (a *authStore) refresh(w http.ResponseWriter, r *http.Request, passwordPath string) {
	if !readForm(w, r) {
		return
	}
	if r.PostForm.Get("client_id") != clientID || r.PostForm.Get("grant_type") != "refresh_token" || r.PostForm.Get("refresh_token") == "" {
		http.Error(w, "invalid grant", http.StatusUnauthorized)
		return
	}
	state, err := readPassword(passwordPath)
	if err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return
	}
	access, refresh, err := a.pair(state.Epoch, r.PostForm.Get("refresh_token"))
	if err != nil {
		http.Error(w, "invalid grant", http.StatusUnauthorized)
		return
	}
	writeTokenResponse(w, access, refresh)
}

func (a *authStore) revokeHandler(w http.ResponseWriter, r *http.Request, passwordPath string) {
	if !readForm(w, r) {
		return
	}
	if r.PostForm.Get("client_id") != clientID || r.PostForm.Get("token_type_hint") != "refresh_token" {
		http.Error(w, "invalid request", http.StatusBadRequest)
		return
	}
	state, err := readPassword(passwordPath)
	if err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return
	}
	if err := a.revoke(state.Epoch, r.PostForm.Get("token")); err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusOK)
}

func (a *authStore) requireAccess(w http.ResponseWriter, r *http.Request, passwordPath string) bool {
	state, err := readPassword(passwordPath)
	if err != nil {
		http.Error(w, "authentication unavailable", http.StatusServiceUnavailable)
		return false
	}
	parts := strings.SplitN(r.Header.Get("Authorization"), " ", 2)
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") || !a.authenticate(parts[1], state.Epoch) {
		w.Header().Set("WWW-Authenticate", "Bearer")
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return false
	}
	return true
}

func requirePasswordReady(dataDir string) error {
	_, err := readPassword(filepath.Join(dataDir, "auth.json"))
	if err != nil {
		return fmt.Errorf("set the service password before startup: %w", err)
	}
	return nil
}
