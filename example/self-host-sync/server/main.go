package main

import (
	"flag"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"time"
)

const (
	clientID      = "norishell-self-host"
	scope         = "ssh.sync"
	serverVersion = "0.1.12"
)

func main() {
	if len(os.Args) == 2 && (os.Args[1] == "--version" || os.Args[1] == "version") {
		fmt.Println("NoriShell Sync Server " + serverVersion)
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "password" {
		passwordCommand(os.Args[2:])
		return
	}
	flags := flag.NewFlagSet("serve", flag.ExitOnError)
	dataDir := flags.String("data-dir", "", "private state directory (default: user configuration directory)")
	listen := flags.String("listen", "", "override saved HTTP bind address (explicitly allows non-loopback)")
	_ = flags.Parse(os.Args[1:])
	dir, err := resolveDataDir(*dataDir)
	if err != nil {
		log.Fatal(err)
	}
	address, err := initializeServer(dir, *listen, os.Stdin, os.Stderr)
	if err != nil {
		log.Fatal(err)
	}
	service, err := newService(dir)
	if err != nil {
		log.Fatal(err)
	}
	server := &http.Server{
		Addr:              address,
		Handler:           service.routes(),
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       40 * time.Second,
		WriteTimeout:      40 * time.Second,
		IdleTimeout:       60 * time.Second,
		MaxHeaderBytes:    16 << 10,
	}
	log.Printf("NoriShell Sync Server %s listening on %s. HTTP exposes the sync password, tokens, and request metadata in transit; use HTTPS for Internet access.", serverVersion, address)
	log.Fatal(server.ListenAndServe())
}

func validateListen(address string) error {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return err
	}
	ip := net.ParseIP(host)
	if ip == nil {
		return fmt.Errorf("listen address must use an IP address")
	}
	if !validPort(port) {
		return fmt.Errorf("listen port must be between 1 and 65535")
	}
	return nil
}

func passwordCommand(args []string) {
	flags := flag.NewFlagSet("password", flag.ExitOnError)
	dataDir := flags.String("data-dir", "", "private state directory (default: user configuration directory)")
	_ = flags.Parse(args)
	if len(flags.Args()) != 1 || flags.Args()[0] != "set" {
		fmt.Fprintln(os.Stderr, "usage: server password --data-dir DIR set < password-on-stdin")
		os.Exit(2)
	}
	dir, err := resolveDataDir(*dataDir)
	if err != nil {
		log.Fatal(err)
	}
	if err := ensureDataDir(dir); err != nil {
		log.Fatal(err)
	}
	if err := setPassword(filepath.Join(dir, "auth.json"), os.Stdin); err != nil {
		log.Fatal(err)
	}
	fmt.Fprintln(os.Stderr, "Service password updated; existing sessions are invalidated.")
}
