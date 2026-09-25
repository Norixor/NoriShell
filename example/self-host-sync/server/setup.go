package main

import (
	"bufio"
	"bytes"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"golang.org/x/term"
)

const defaultListen = "127.0.0.1:8787"

type serverConfig struct {
	Version int    `json:"version"`
	Listen  string `json:"listen"`
}

func validPort(value string) bool {
	port, err := strconv.Atoi(value)
	return err == nil && port >= 1 && port <= 65535
}

func resolveDataDir(explicit string) (string, error) {
	if explicit != "" {
		return explicit, nil
	}
	base, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	current := filepath.Join(base, "NoriShell", "self-host-sync")
	if _, err := os.Stat(filepath.Join(current, "auth.json")); err == nil {
		return current, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	if _, err := os.Stat(filepath.Join(current, "config.json")); err == nil {
		return current, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	// Preserve installations that used the previous ./data default.
	legacy := filepath.Join(".", "data")
	if _, err := os.Stat(filepath.Join(legacy, "auth.json")); err == nil {
		return legacy, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	return current, nil
}

func ensureDataDir(dir string) error {
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	info, err := os.Lstat(dir)
	if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("data directory must be a real directory")
	}
	return os.Chmod(dir, 0700)
}

func readServerConfig(path string) (serverConfig, error) {
	var config serverConfig
	data, err := os.ReadFile(path)
	if err != nil {
		return config, err
	}
	if len(data) > 1024 || json.Unmarshal(data, &config) != nil || config.Version != 1 || validateListen(config.Listen) != nil {
		return serverConfig{}, errors.New("invalid server configuration")
	}
	return config, nil
}

func saveServerConfig(path, address string) error {
	if err := validateListen(address); err != nil {
		return err
	}
	data, err := json.Marshal(serverConfig{Version: 1, Listen: address})
	if err != nil {
		return err
	}
	committed, err := writeAtomic(path, data)
	if committed && err != nil {
		return fmt.Errorf("server configuration may have committed; inspect config.json before retrying: %w", err)
	}
	return err
}

func initializeServer(dir, override string, input *os.File, output io.Writer) (string, error) {
	if err := ensureDataDir(dir); err != nil {
		return "", err
	}
	if override != "" {
		if err := validateListen(override); err != nil {
			return "", err
		}
	}
	configPath := filepath.Join(dir, "config.json")
	config, configErr := readServerConfig(configPath)
	if configErr != nil && !errors.Is(configErr, os.ErrNotExist) {
		return "", configErr
	}
	passwordPath := filepath.Join(dir, "auth.json")
	_, passwordErr := readPassword(passwordPath)
	needsPassword := errors.Is(passwordErr, os.ErrNotExist)
	if passwordErr != nil && !needsPassword {
		return "", passwordErr
	}
	address := config.Listen
	if override != "" {
		address = override
	}
	interactive := term.IsTerminal(int(input.Fd()))
	if errors.Is(configErr, os.ErrNotExist) {
		if address == "" {
			address = defaultListen
			if needsPassword && interactive {
				port, err := promptPort(input, output)
				if err != nil {
					return "", err
				}
				address = net.JoinHostPort("127.0.0.1", port)
			}
		}
		if needsPassword && !interactive {
			return "", errors.New("first run needs a terminal; for unattended setup run 'password set' with the password on stdin, then start with --listen IP:PORT")
		}
		if err := saveServerConfig(configPath, address); err != nil {
			return "", err
		}
	}
	if needsPassword {
		if !interactive {
			return "", errors.New("service password missing; run 'password set' with the password on stdin")
		}
		if err := promptAndSetPassword(passwordPath, input, output); err != nil {
			return "", err
		}
	}
	return address, nil
}

func promptPort(input io.Reader, output io.Writer) (string, error) {
	fmt.Fprint(output, "Bind port on 127.0.0.1 [8787]: ")
	line, err := bufio.NewReader(io.LimitReader(input, 16)).ReadString('\n')
	if err != nil {
		return "", fmt.Errorf("read bind port: %w", err)
	}
	port := strings.TrimSpace(line)
	if port == "" {
		port = "8787"
	}
	if !validPort(port) {
		return "", errors.New("bind port must be between 1 and 65535")
	}
	return port, nil
}

func promptAndSetPassword(path string, input *os.File, output io.Writer) error {
	fmt.Fprint(output, "Sync password (12-1024 bytes): ")
	first, err := term.ReadPassword(int(input.Fd()))
	fmt.Fprintln(output)
	if err != nil {
		return fmt.Errorf("read password: %w", err)
	}
	defer clearBytes(first)
	fmt.Fprint(output, "Confirm sync password: ")
	second, err := term.ReadPassword(int(input.Fd()))
	fmt.Fprintln(output)
	if err != nil {
		return fmt.Errorf("confirm password: %w", err)
	}
	defer clearBytes(second)
	if subtle.ConstantTimeCompare(first, second) != 1 {
		return errors.New("passwords do not match")
	}
	if err := setPassword(path, bytes.NewReader(first)); err != nil {
		return err
	}
	fmt.Fprintln(output, "Setup complete. For other devices, bind explicitly with --listen 0.0.0.0:PORT and enter the server's IP in the plugin. HTTP exposes the sync password, tokens, and request metadata in transit; use HTTPS for Internet access.")
	return nil
}

func clearBytes(value []byte) {
	for index := range value {
		value[index] = 0
	}
}
