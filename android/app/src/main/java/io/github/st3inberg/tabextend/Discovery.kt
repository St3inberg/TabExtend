package io.github.st3inberg.tabextend

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import java.net.Inet4Address
import java.util.concurrent.Executors

/** A computer announcing TabExtend on the local network. */
data class FoundComputer(
    val name: String,
    val address: String,
    val port: Int,
    /** Short id of the host certificate; lets a paired tablet find its host again. */
    val id: String,
    /** Open for pairing right now (`tabextend pair` is running). */
    val pairing: Boolean,
)

/**
 * Lists computers running TabExtend, via the host's mDNS announcement
 * (`_tabextend._tcp`, see crates/host/src/mdns.rs). Results arrive on a
 * background thread.
 */
class Discovery(context: Context, private val onChange: (List<FoundComputer>) -> Unit) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val found = linkedMapOf<String, FoundComputer>()
    private val executor = Executors.newSingleThreadExecutor()
    /** Android 14+ only; stays empty on older versions. */
    private val callbacks = mutableMapOf<String, Any>()
    private var listener: NsdManager.DiscoveryListener? = null

    fun start() {
        if (listener != null) return
        val discovery = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(info: NsdServiceInfo) = resolve(info)
            override fun onServiceLost(info: NsdServiceInfo) = remove(info.serviceName)
            override fun onDiscoveryStarted(type: String) {}
            override fun onDiscoveryStopped(type: String) {}
            override fun onStartDiscoveryFailed(type: String, error: Int) {}
            override fun onStopDiscoveryFailed(type: String, error: Int) {}
        }
        listener = discovery
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discovery)
    }

    fun stop() {
        listener?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        listener = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            callbacks.values.forEach {
                runCatching { nsd.unregisterServiceInfoCallback(it as NsdManager.ServiceInfoCallback) }
            }
        }
        callbacks.clear()
        synchronized(found) { found.clear() }
    }

    private fun resolve(info: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val callback = object : NsdManager.ServiceInfoCallback {
                override fun onServiceUpdated(resolved: NsdServiceInfo) = add(resolved)
                override fun onServiceLost() = remove(info.serviceName)
                override fun onServiceInfoCallbackRegistrationFailed(error: Int) {}
                override fun onServiceInfoCallbackUnregistered() {}
            }
            callbacks[info.serviceName] = callback
            nsd.registerServiceInfoCallback(info, executor, callback)
        } else {
            @Suppress("DEPRECATION")
            nsd.resolveService(info, object : NsdManager.ResolveListener {
                override fun onServiceResolved(resolved: NsdServiceInfo) = add(resolved)
                override fun onResolveFailed(info: NsdServiceInfo, error: Int) {}
            })
        }
    }

    private fun add(info: NsdServiceInfo) {
        val address = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            info.hostAddresses.firstOrNull { it is Inet4Address } ?: info.hostAddresses.firstOrNull()
        } else {
            @Suppress("DEPRECATION")
            info.host
        } ?: return
        val txt = info.attributes.mapValues { (_, v) -> v?.toString(Charsets.UTF_8) ?: "" }
        if (txt["v"] != "1") return
        val computer = FoundComputer(
            name = info.serviceName.removePrefix("TabExtend on ").removePrefix("TabExtend "),
            address = address.hostAddress?.substringBefore('%') ?: return,
            port = info.port,
            id = txt["id"].orEmpty(),
            pairing = txt["pair"] == "1",
        )
        publish { it[info.serviceName] = computer }
    }

    private fun remove(serviceName: String) = publish { it.remove(serviceName) }

    private fun publish(change: (MutableMap<String, FoundComputer>) -> Unit) {
        val snapshot = synchronized(found) {
            change(found)
            found.values.toList()
        }
        onChange(snapshot)
    }

    private companion object {
        const val SERVICE_TYPE = "_tabextend._tcp"
    }
}
