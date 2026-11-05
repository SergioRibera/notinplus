package dev.istmo.plugins.installer

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import dev.istmo.runtime.BackendException
import dev.istmo.runtime.InstallError
import dev.istmo.runtime.InstallerBackend
import java.io.File

/**
 * Reference backend for `istmo.installer`: the system package
 * installer, fed a [IstmoInstallerFileProvider] URI.
 */
class InstallerBackendImpl(
    private val context: Context,
) : InstallerBackend {

    override suspend fun install(path: String) {
        val file = File(path)
        if (!file.isFile) {
            throw BackendException(InstallError.NotFound)
        }
        if (!context.packageManager.canRequestPackageInstalls()) {
            val settings = Intent(
                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${context.packageName}"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(settings) }
            throw BackendException(InstallError.NeedsPermission)
        }
        val uri = try {
            FileProvider.getUriForFile(context, "${context.packageName}$AUTHORITY_SUFFIX", file)
        } catch (e: IllegalArgumentException) {
            throw BackendException(InstallError.Backend("FileProvider cannot expose ${file.path}: ${e.message}"))
        }
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, APK_MIME)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            throw BackendException(InstallError.Unsupported)
        } catch (e: SecurityException) {
            throw BackendException(InstallError.Backend(e.message ?: "not allowed"))
        }
    }

    private companion object {
        const val AUTHORITY_SUFFIX = ".istmo.installer.files"
        const val APK_MIME = "application/vnd.android.package-archive"
    }
}

/**
 * The plugin's own provider class, so it never collides with another
 * `FileProvider` the app or a plugin declares.
 */
class IstmoInstallerFileProvider : FileProvider()
