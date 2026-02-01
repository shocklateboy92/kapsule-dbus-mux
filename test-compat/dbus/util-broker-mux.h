/*
 * Test utilities for kapsule-dbus-mux
 * 
 * This is a simplified header-only utility for running dbus-broker-style
 * tests against kapsule-dbus-mux with minimal dependencies.
 * 
 * Only requires: libsystemd-dev
 */

#ifndef UTIL_BROKER_MUX_H
#define UTIL_BROKER_MUX_H

#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <signal.h>
#include <time.h>
#include <assert.h>
#include <systemd/sd-bus.h>
#include <systemd/sd-event.h>

/* Assert macro compatible with c-stdaux */
#define c_assert(x) assert(x)

/* Cleanup macros compatible with c-stdaux */
#define _c_cleanup_(f) __attribute__((__cleanup__(f)))

/* Forward declarations */
static void util_broker_free(void *broker);

static inline void util_broker_freep(void *p) {
    void **pp = (void **)p;
    if (*pp) {
        util_broker_free(*pp);
        *pp = NULL;
    }
}

/*
 * Broker - manages dbus-daemon + kapsule-dbus-mux for testing
 */
typedef struct Broker {
    /* Backing dbus-daemon */
    pid_t backing_daemon_pid;
    char *backing_address;      /* unix:path=/tmp/xxx.sock */
    char *backing_socket_path;  /* /tmp/xxx.sock */
    
    /* kapsule-dbus-mux */
    pid_t mux_pid;
    char *mux_socket_path;      /* Path clients connect to */
    
    /* Runtime directory for sockets */
    char *runtime_dir;
    
    /* Path to mux binary (from DBUS_BROKER_TEST_MUX env) */
    char *mux_binary;
    
    /* Event loop for async operations */
    sd_event *event;
    
    /* State flags */
    bool spawned;
} Broker;

/*
 * Generate a unique socket path for this test instance
 */
static inline char *util_generate_socket_path(const char *prefix) {
    char *path;
    static int counter = 0;
    int r;
    
    r = asprintf(&path, "/tmp/%s-%d-%ld-%d.sock", 
                 prefix, getpid(), (long)time(NULL), counter++);
    if (r < 0) {
        return NULL;
    }
    return path;
}

/*
 * Generate unique runtime directory
 */
static inline char *util_generate_runtime_dir(void) {
    char *path;
    int r;
    
    r = asprintf(&path, "/tmp/dbus-mux-test-%d-%ld", 
                 getpid(), (long)time(NULL));
    if (r < 0) {
        return NULL;
    }
    
    if (mkdir(path, 0700) < 0 && errno != EEXIST) {
        free(path);
        return NULL;
    }
    
    return path;
}

/*
 * Wait for a socket to become available
 */
static inline int util_wait_for_socket(const char *path, int timeout_ms) {
    int elapsed = 0;
    struct stat st;
    
    while (elapsed < timeout_ms) {
        if (stat(path, &st) == 0 && S_ISSOCK(st.st_mode)) {
            /* Socket exists, try to connect */
            int fd = socket(AF_UNIX, SOCK_STREAM, 0);
            if (fd >= 0) {
                struct sockaddr_un addr = { .sun_family = AF_UNIX };
                strncpy(addr.sun_path, path, sizeof(addr.sun_path) - 1);
                
                if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) {
                    close(fd);
                    return 0;
                }
                close(fd);
            }
        }
        usleep(10000); /* 10ms */
        elapsed += 10;
    }
    
    return -ETIMEDOUT;
}

/*
 * Fork and exec the backing dbus-daemon
 */
static inline int util_fork_backing_daemon(Broker *broker) {
    int r;
    
    broker->backing_socket_path = util_generate_socket_path("dbus-backing");
    if (!broker->backing_socket_path) {
        return -ENOMEM;
    }
    
    r = asprintf(&broker->backing_address, "unix:path=%s", 
                 broker->backing_socket_path);
    if (r < 0) {
        return -ENOMEM;
    }
    
    broker->backing_daemon_pid = fork();
    if (broker->backing_daemon_pid < 0) {
        return -errno;
    }
    
    if (broker->backing_daemon_pid == 0) {
        /* Child: exec dbus-daemon */
        execlp("dbus-daemon", "dbus-daemon",
               "--session",
               "--nofork",
               "--nopidfile",
               "--nosyslog",
               "--address", broker->backing_address,
               (char *)NULL);
        _exit(127);
    }
    
    /* Parent: wait for socket */
    r = util_wait_for_socket(broker->backing_socket_path, 5000);
    if (r < 0) {
        fprintf(stderr, "ERROR: Failed to start backing dbus-daemon\n");
        kill(broker->backing_daemon_pid, SIGTERM);
        waitpid(broker->backing_daemon_pid, NULL, 0);
        broker->backing_daemon_pid = 0;
        return r;
    }
    
    return 0;
}

/*
 * Fork and exec kapsule-dbus-mux
 */
static inline int util_fork_mux(Broker *broker) {
    int r;
    
    broker->mux_socket_path = util_generate_socket_path("dbus-mux");
    if (!broker->mux_socket_path) {
        return -ENOMEM;
    }
    
    broker->mux_pid = fork();
    if (broker->mux_pid < 0) {
        return -errno;
    }
    
    if (broker->mux_pid == 0) {
        /* Child: exec kapsule-dbus-mux 
         * kapsule-dbus-mux uses:
         *   --listen/-l <socket_path>  - Socket for clients to connect to
         *   --container-bus/-c <address> - The "container" bus (our backing daemon)
         * 
         * In test mode, we treat the backing dbus-daemon as the "container bus"
         * and don't set a host bus.
         */
        execl(broker->mux_binary, broker->mux_binary,
              "--listen", broker->mux_socket_path,
              "--container-bus", broker->backing_address,
              "--log-level", "debug",
              (char *)NULL);
        
        fprintf(stderr, "ERROR: Failed to exec mux: %s\n", strerror(errno));
        _exit(127);
    }
    
    /* Parent: wait for socket */
    r = util_wait_for_socket(broker->mux_socket_path, 5000);
    if (r < 0) {
        fprintf(stderr, "ERROR: Failed to start kapsule-dbus-mux (socket not ready)\n");
        kill(broker->mux_pid, SIGTERM);
        waitpid(broker->mux_pid, NULL, 0);
        broker->mux_pid = 0;
        return r;
    }
    
    return 0;
}

/*
 * Create a new broker instance
 */
static inline int util_broker_new(Broker **brokerp) {
    Broker *broker;
    const char *mux_path;
    int r;
    
    broker = calloc(1, sizeof(*broker));
    if (!broker) {
        return -ENOMEM;
    }
    
    /* Get mux binary path from environment */
    mux_path = getenv("DBUS_BROKER_TEST_MUX");
    if (!mux_path) {
        fprintf(stderr, "ERROR: DBUS_BROKER_TEST_MUX environment variable not set\n");
        fprintf(stderr, "Set it to the path of your kapsule-dbus-mux binary\n");
        free(broker);
        return -EINVAL;
    }
    
    /* Verify binary exists */
    if (access(mux_path, X_OK) != 0) {
        fprintf(stderr, "ERROR: MUX binary not executable: %s\n", mux_path);
        free(broker);
        return -ENOENT;
    }
    
    broker->mux_binary = strdup(mux_path);
    if (!broker->mux_binary) {
        free(broker);
        return -ENOMEM;
    }
    
    /* Create event loop */
    r = sd_event_default(&broker->event);
    if (r < 0) {
        free(broker->mux_binary);
        free(broker);
        return r;
    }
    
    /* Create runtime directory */
    broker->runtime_dir = util_generate_runtime_dir();
    
    *brokerp = broker;
    return 0;
}

/*
 * Spawn the backing daemon and mux
 */
static inline void util_broker_spawn(Broker *broker) {
    int r;
    
    c_assert(broker);
    c_assert(!broker->spawned);
    
    /* Start backing dbus-daemon */
    r = util_fork_backing_daemon(broker);
    if (r < 0) {
        fprintf(stderr, "FATAL: Failed to start backing daemon: %d\n", r);
    }
    c_assert(r >= 0);
    
    /* Start kapsule-dbus-mux */
    r = util_fork_mux(broker);
    if (r < 0) {
        fprintf(stderr, "FATAL: Failed to start mux: %d\n", r);
    }
    c_assert(r >= 0);
    
    broker->spawned = true;
    
    fprintf(stderr, "[TEST] Broker spawned successfully\n");
    fprintf(stderr, "       MUX socket: %s\n", broker->mux_socket_path);
    fprintf(stderr, "       Backing: %s\n", broker->backing_socket_path);
}

/*
 * Connect an sd_bus to the mux (performs Hello automatically)
 */
static inline void util_broker_connect(Broker *broker, sd_bus **busp) {
    _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
    char *address = NULL;
    int r;
    
    c_assert(broker);
    c_assert(broker->spawned);
    
    r = sd_bus_new(&bus);
    c_assert(r >= 0);
    
    r = asprintf(&address, "unix:path=%s", broker->mux_socket_path);
    c_assert(r >= 0);
    
    r = sd_bus_set_address(bus, address);
    free(address);
    c_assert(r >= 0);
    
    r = sd_bus_set_bus_client(bus, true);
    c_assert(r >= 0);
    
    r = sd_bus_start(bus);
    if (r < 0) {
        fprintf(stderr, "ERROR: Failed to connect to mux: %s\n", strerror(-r));
    }
    c_assert(r >= 0);
    
    *busp = bus;
    bus = NULL;
}

/*
 * Connect without doing Hello (for testing Hello behavior)
 */
static inline void util_broker_connect_raw(Broker *broker, sd_bus **busp) {
    _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
    char *address = NULL;
    int r;
    
    c_assert(broker);
    c_assert(broker->spawned);
    
    r = sd_bus_new(&bus);
    c_assert(r >= 0);
    
    r = asprintf(&address, "unix:path=%s", broker->mux_socket_path);
    c_assert(r >= 0);
    
    r = sd_bus_set_address(bus, address);
    free(address);
    c_assert(r >= 0);
    
    /* Don't automatically call Hello - set as anonymous peer */
    r = sd_bus_set_bus_client(bus, false);
    c_assert(r >= 0);
    
    r = sd_bus_start(bus);
    if (r < 0) {
        fprintf(stderr, "ERROR: Failed to connect raw to mux: %s\n", strerror(-r));
    }
    c_assert(r >= 0);
    
    *busp = bus;
    bus = NULL;
}

/*
 * Terminate the broker and cleanup
 */
static inline void util_broker_terminate(Broker *broker) {
    c_assert(broker);
    
    if (broker->mux_pid > 0) {
        kill(broker->mux_pid, SIGTERM);
        waitpid(broker->mux_pid, NULL, 0);
        broker->mux_pid = 0;
    }
    
    if (broker->backing_daemon_pid > 0) {
        kill(broker->backing_daemon_pid, SIGTERM);
        waitpid(broker->backing_daemon_pid, NULL, 0);
        broker->backing_daemon_pid = 0;
    }
    
    if (broker->mux_socket_path) {
        unlink(broker->mux_socket_path);
    }
    
    if (broker->backing_socket_path) {
        unlink(broker->backing_socket_path);
    }
    
    broker->spawned = false;
    
    fprintf(stderr, "[TEST] Broker terminated\n");
}

/*
 * Free broker and all resources
 */
static void util_broker_free(void *p) {
    Broker *broker = p;
    
    if (!broker) {
        return;
    }
    
    if (broker->spawned) {
        util_broker_terminate(broker);
    }
    
    if (broker->event) {
        sd_event_unref(broker->event);
    }
    
    free(broker->mux_binary);
    free(broker->runtime_dir);
    free(broker->backing_address);
    free(broker->backing_socket_path);
    free(broker->mux_socket_path);
    free(broker);
}

/*
 * c_freep - cleanup helper for free()
 */
static inline void c_freep(void *p) {
    void **pp = (void **)p;
    free(*pp);
}

/*
 * Consume a signal from the bus, verify interface and member
 */
static inline void util_broker_consume_signal(sd_bus *bus, const char *interface, const char *member) {
    int r;
    
    for (;;) {
        _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *message = NULL;
        
        r = sd_bus_process(bus, &message);
        c_assert(r >= 0);
        
        if (!message) {
            /* No message ready, wait for one */
            r = sd_bus_wait(bus, 1000000); /* 1 second timeout */
            c_assert(r >= 0);
            continue;
        }
        
        if (sd_bus_message_is_signal(message, interface, member)) {
            return;
        }
        
        /* Skip other messages (method returns, etc) */
    }
}

/*
 * Consume a method call from the bus
 */
static inline void util_broker_consume_method_call(sd_bus *bus, const char *interface, const char *member) {
    int r;
    
    for (;;) {
        _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *message = NULL;
        
        r = sd_bus_process(bus, &message);
        c_assert(r >= 0);
        
        if (!message) {
            r = sd_bus_wait(bus, 1000000);
            c_assert(r >= 0);
            continue;
        }
        
        if (sd_bus_message_is_method_call(message, interface, member)) {
            return;
        }
    }
}

/*
 * Consume a method return from the bus
 */
static inline void util_broker_consume_method_return(sd_bus *bus) {
    int r;
    
    for (;;) {
        _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *message = NULL;
        uint8_t type;
        
        r = sd_bus_process(bus, &message);
        c_assert(r >= 0);
        
        if (!message) {
            r = sd_bus_wait(bus, 1000000);
            c_assert(r >= 0);
            continue;
        }
        
        r = sd_bus_message_get_type(message, &type);
        c_assert(r >= 0);
        
        if (type == SD_BUS_MESSAGE_METHOD_RETURN) {
            return;
        }
    }
}

/*
 * Consume a method error from the bus
 */
static inline void util_broker_consume_method_error(sd_bus *bus, const char *name) {
    int r;
    
    for (;;) {
        _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *message = NULL;
        uint8_t type;
        
        r = sd_bus_process(bus, &message);
        c_assert(r >= 0);
        
        if (!message) {
            r = sd_bus_wait(bus, 1000000);
            c_assert(r >= 0);
            continue;
        }
        
        r = sd_bus_message_get_type(message, &type);
        c_assert(r >= 0);
        
        if (type == SD_BUS_MESSAGE_METHOD_ERROR) {
            const sd_bus_error *error = sd_bus_message_get_error(message);
            c_assert(error);
            c_assert(strcmp(error->name, name) == 0);
            return;
        }
    }
}

/*
 * Create a new event loop
 */
static inline void util_event_new(sd_event **eventp) {
    int r;
    
    r = sd_event_new(eventp);
    c_assert(r >= 0);
}

#endif /* UTIL_BROKER_MUX_H */
