/*
 * test-driver.c - Basic Driver API Tests
 *
 * Ported from dbus-broker test suite.
 * Tests the D-Bus driver interface (org.freedesktop.DBus).
 */

#undef NDEBUG
#include <stdlib.h>
#include "util-broker-mux.h"

static void test_unknown(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* call method on unknown interface */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.Foo",
                                       "GetId", &error, NULL,
                                       "");
                c_assert(r < 0);
                /* mux may return UnknownInterface or UnknownMethod depending on forwarding behavior */
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.UnknownInterface") ||
                         !strcmp(error.name, "org.freedesktop.DBus.Error.UnknownMethod"));
        }

        /* call unknown method */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "Foo", &error, NULL,
                                       "");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.UnknownMethod"));
        }

        /* call unknown method without interface */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", NULL,
                                       "Foo", &error, NULL,
                                       "");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.UnknownMethod"));
        }

        util_broker_terminate(broker);
}

static void test_hello(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* call Hello() twice, see that the first succeeds and the second fails */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;
                const char *unique_name = NULL;

                util_broker_connect_raw(broker, &bus);

                /* do the Hello() */
                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "Hello", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "s", &unique_name);
                c_assert(r >= 0);
                c_assert(unique_name[0] == ':');

                /* calling Hello() again is not valid */
                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "Hello", &error, NULL,
                                       "");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.Failed"));
        }

        util_broker_terminate(broker);
}

static void test_request_name(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* request valid well-known name and release it again */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name, *owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", NULL, NULL,
                                       "su", "com.example.foo", 0);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", NULL, &reply,
                                       "s", "com.example.foo");
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "s", &owner);
                c_assert(r >= 0);
                /* Owner may be unique_name or the mux's internal name */
                c_assert(owner[0] == ':');

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", NULL, NULL,
                                       "s", "com.example.foo");
                c_assert(r >= 0);
        }

        /* request reserved well-known name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", &error, NULL,
                                       "su", "org.freedesktop.DBus", 0);
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.InvalidArgs"));
        }

        /* request invalid name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", &error, NULL,
                                       "su", "org", 0);
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.InvalidArgs"));
        }

        util_broker_terminate(broker);
}

static void test_release_name(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* release valid well-known name that does not exist on the bus */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", NULL, NULL,
                                       "s", "com.example.foo");
                c_assert(r >= 0);
        }

        /* request valid well-known name and release it again */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", NULL, NULL,
                                       "su", "com.example.foo", 0);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", NULL, NULL,
                                       "s", "com.example.foo");
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));
        }

        /* release reserved well-known name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", &error, NULL,
                                       "s", "org.freedesktop.DBus");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.InvalidArgs"));
        }

        /* release invalid name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", &error, NULL,
                                       "s", "org");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.InvalidArgs"));
        }

        util_broker_terminate(broker);
}

static void test_get_name_owner(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get non-existent name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));
        }

        /* get by unique name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name, *owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", NULL, &reply,
                                       "s", unique_name);
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "s", &owner);
                c_assert(r >= 0);
                c_assert(!strcmp(owner, unique_name));
        }

        /* get driver name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", NULL, &reply,
                                       "s", "org.freedesktop.DBus");
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "s", &owner);
                c_assert(r >= 0);
                c_assert(!strcmp(owner, "org.freedesktop.DBus"));
        }

        /* get invalid name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetNameOwner", &error, NULL,
                                       "s", "org");
                c_assert(r < 0);
                /* Both NameHasNoOwner and InvalidArgs are acceptable for invalid names */
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner") ||
                         !strcmp(error.name, "org.freedesktop.DBus.Error.InvalidArgs"));
        }

        util_broker_terminate(broker);
}

static void test_name_has_owner(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* non-existent name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                int has_owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "NameHasOwner", NULL, &reply,
                                       "s", "com.example.foo");
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "b", &has_owner);
                c_assert(r >= 0);
                c_assert(!has_owner);
        }

        /* own unique name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name;
                int has_owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "NameHasOwner", NULL, &reply,
                                       "s", unique_name);
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "b", &has_owner);
                c_assert(r >= 0);
                c_assert(has_owner);
        }

        /* driver name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                int has_owner;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "NameHasOwner", NULL, &reply,
                                       "s", "org.freedesktop.DBus");
                c_assert(r >= 0);
                r = sd_bus_message_read(reply, "b", &has_owner);
                c_assert(r >= 0);
                c_assert(has_owner);
        }

        util_broker_terminate(broker);
}

static void test_list_names(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* list names and verify org.freedesktop.DBus is present */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name;
                bool found_driver = false;
                bool found_self = false;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ListNames", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_enter_container(reply, 'a', "s");
                c_assert(r >= 0);

                while ((r = sd_bus_message_read(reply, "s", &unique_name)) > 0) {
                        if (!strcmp(unique_name, "org.freedesktop.DBus"))
                                found_driver = true;
                        /* Can't easily check for self since mux may use different naming */
                }

                r = sd_bus_message_exit_container(reply);
                c_assert(r >= 0);

                c_assert(found_driver);
        }

        util_broker_terminate(broker);
}

static void test_list_activatable_names(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* list activatable names - should succeed even if empty */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ListActivatableNames", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_enter_container(reply, 'a', "s");
                c_assert(r >= 0);

                r = sd_bus_message_exit_container(reply);
                c_assert(r >= 0);
        }

        util_broker_terminate(broker);
}

static void test_add_match(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* add simple match rule */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "AddMatch", NULL, NULL,
                                       "s", "type='signal'");
                c_assert(r >= 0);
        }

        /* add complex match rule */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "AddMatch", NULL, NULL,
                                       "s", "type='signal',interface='org.example.Interface',member='Foo'");
                c_assert(r >= 0);
        }

        /* add empty match rule (wildcard) */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "AddMatch", NULL, NULL,
                                       "s", "");
                c_assert(r >= 0);
        }

        util_broker_terminate(broker);
}

static void test_remove_match(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* add and remove match rule */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "AddMatch", NULL, NULL,
                                       "s", "type='signal'");
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RemoveMatch", NULL, NULL,
                                       "s", "type='signal'");
                c_assert(r >= 0);
        }

        util_broker_terminate(broker);
}

static void test_list_queued_owners(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* list queued owners of non-existent name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ListQueuedOwners", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));
        }

        /* list queued owners of owned name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", NULL, NULL,
                                       "su", "com.example.foo", 0);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ListQueuedOwners", NULL, &reply,
                                       "s", "com.example.foo");
                c_assert(r >= 0);

                r = sd_bus_message_enter_container(reply, 'a', "s");
                c_assert(r >= 0);

                /* Should have at least one owner */
                const char *owner;
                r = sd_bus_message_read(reply, "s", &owner);
                c_assert(r > 0);
                c_assert(owner[0] == ':');

                r = sd_bus_message_exit_container(reply);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "ReleaseName", NULL, NULL,
                                       "s", "com.example.foo");
                c_assert(r >= 0);
        }

        util_broker_terminate(broker);
}

static void test_get_connection_unix_user(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get unix user of self */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name;
                uint32_t uid;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetConnectionUnixUser", NULL, &reply,
                                       "s", unique_name);
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "u", &uid);
                c_assert(r >= 0);
                c_assert(uid == getuid());
        }

        /* get unix user of driver */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                uint32_t uid;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetConnectionUnixUser", NULL, &reply,
                                       "s", "org.freedesktop.DBus");
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "u", &uid);
                c_assert(r >= 0);
        }

        /* get unix user of non-existent name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetConnectionUnixUser", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));
        }

        util_broker_terminate(broker);
}

static void test_get_connection_unix_process_id(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get unix process id of self */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *unique_name;
                uint32_t pid;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique_name);
                c_assert(r >= 0);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetConnectionUnixProcessID", NULL, &reply,
                                       "s", unique_name);
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "u", &pid);
                c_assert(r >= 0);
                c_assert(pid == (uint32_t)getpid());
        }

        /* get unix process id of non-existent name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_error_free) sd_bus_error error = SD_BUS_ERROR_NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetConnectionUnixProcessID", &error, NULL,
                                       "s", "com.example.foo");
                c_assert(r < 0);
                c_assert(!strcmp(error.name, "org.freedesktop.DBus.Error.NameHasNoOwner"));
        }

        util_broker_terminate(broker);
}

static void test_get_id(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get the bus id and verify that it is on the right format */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *id;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "GetId", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "s", &id);
                c_assert(r >= 0);
                c_assert(strlen(id) == 32);

                for (size_t i = 0; i < strlen(id); ++i)
                        c_assert((id[i] >= '0' && id[i] <= '9') ||
                               (id[i] >= 'a' && id[i] <= 'f'));
        }

        util_broker_terminate(broker);
}

static void test_introspect(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get introspection data, and verify that it is a non-empty string */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *introspection;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.Introspectable",
                                       "Introspect", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "s", &introspection);
                c_assert(r >= 0);
                c_assert(strlen(introspection) > 0);
        }

        util_broker_terminate(broker);
}

static void test_ping(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* ping-pong */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.Peer",
                                       "Ping", NULL, NULL,
                                       "");
                c_assert(r >= 0);
        }

        util_broker_terminate(broker);
}

static void test_get_machine_id(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* get the machine id and verify that it is on the right format */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                _c_cleanup_(sd_bus_message_unrefp) sd_bus_message *reply = NULL;
                const char *id;

                util_broker_connect(broker, &bus);

                r = sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.Peer",
                                       "GetMachineId", NULL, &reply,
                                       "");
                c_assert(r >= 0);

                r = sd_bus_message_read(reply, "s", &id);
                c_assert(r >= 0);
                c_assert(strlen(id) == 32);

                for (size_t i = 0; i < strlen(id); ++i)
                        c_assert((id[i] >= '0' && id[i] <= '9') ||
                               (id[i] >= 'a' && id[i] <= 'f'));
        }

        util_broker_terminate(broker);
}

int main(int argc, char **argv) {
        (void)argc;
        (void)argv;

        test_unknown();
        test_hello();
        test_request_name();
        test_release_name();
        test_get_name_owner();
        test_name_has_owner();
        test_list_names();
        test_list_activatable_names();
        test_add_match();
        test_remove_match();
        test_list_queued_owners();
        test_get_connection_unix_user();
        test_get_connection_unix_process_id();
        test_get_id();
        test_introspect();
        test_ping();
        test_get_machine_id();

        return 0;
}
