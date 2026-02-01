/*
 * test-lifetime.c - Client Lifetime Tests
 *
 * Ported from dbus-broker test suite.
 * Tests client connection and disconnection behavior.
 */

#undef NDEBUG
#include <stdlib.h>
#include "util-broker-mux.h"

static void test_dummy(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;

        util_broker_new(&broker);
        util_broker_spawn(broker);
        util_broker_terminate(broker);
}

static void test_client1(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* simple connect/disconnect */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus = NULL;
                const char *unique = NULL;

                util_broker_connect(broker, &bus);

                r = sd_bus_get_unique_name(bus, &unique);
                c_assert(!r);
                c_assert(unique);
        }

        util_broker_terminate(broker);
}

static void test_client2(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        /* two clients connecting simultaneously */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus1 = NULL;
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *bus2 = NULL;
                const char *unique1 = NULL, *unique2 = NULL;

                util_broker_connect(broker, &bus1);
                util_broker_connect(broker, &bus2);

                r = sd_bus_get_unique_name(bus1, &unique1);
                c_assert(!r);
                c_assert(unique1);

                r = sd_bus_get_unique_name(bus2, &unique2);
                c_assert(!r);
                c_assert(unique2);

                /* verify they have different names */
                c_assert(strcmp(unique1, unique2) != 0);
        }

        util_broker_terminate(broker);
}

static void test_monitor(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *monitor = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        util_broker_connect(broker, &monitor);

        /* subscribe to NameOwnerChanged signals */
        r = sd_bus_call_method(monitor, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                               "AddMatch", NULL, NULL,
                               "s", "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged'");
        c_assert(r >= 0);

        /* create and destroy a client, verify we see NameOwnerChanged */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *client = NULL;

                util_broker_connect(broker, &client);
        }

        /* We should receive at least one NameOwnerChanged for the client connecting/disconnecting */
        util_broker_consume_signal(monitor, "org.freedesktop.DBus", "NameOwnerChanged");

        util_broker_terminate(broker);
}

static void test_names(void) {
        _c_cleanup_(util_broker_freep) Broker *broker = NULL;
        _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *monitor = NULL;
        int r;

        util_broker_new(&broker);
        util_broker_spawn(broker);

        util_broker_connect(broker, &monitor);

        /* subscribe to NameOwnerChanged for a specific name */
        r = sd_bus_call_method(monitor, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                               "AddMatch", NULL, NULL,
                               "s", "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='com.example.test'");
        c_assert(r >= 0);

        /* acquire the name */
        {
                _c_cleanup_(sd_bus_flush_close_unrefp) sd_bus *client = NULL;

                util_broker_connect(broker, &client);

                r = sd_bus_call_method(client, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                                       "RequestName", NULL, NULL,
                                       "su", "com.example.test", 0);
                c_assert(r >= 0);

                /* Should see NameOwnerChanged for acquisition */
                util_broker_consume_signal(monitor, "org.freedesktop.DBus", "NameOwnerChanged");
        }

        /* Client disconnected, should see NameOwnerChanged for release */
        util_broker_consume_signal(monitor, "org.freedesktop.DBus", "NameOwnerChanged");

        util_broker_terminate(broker);
}

int main(int argc, char **argv) {
        (void)argc;
        (void)argv;

        test_dummy();
        test_client1();
        test_client2();
        test_monitor();
        test_names();

        return 0;
}
