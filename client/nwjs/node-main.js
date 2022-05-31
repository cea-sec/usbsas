window.nw.App.clearCache();
chrome.webNavigation.onErrorOccurred.addListener(function (details) {
    chrome.tabs.update(details.tabId, {
        url: chrome.extension.getURL("main.html")
    });
});
