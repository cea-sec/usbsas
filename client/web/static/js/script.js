"use strict";

// État interne
//   - INIT
//   - WAIT_SOURCE
//   - WAIT_DESTINATION
//   - WAIT_ID
//   - SELECT_PARTITION
//   - SELECT_FILES
//   - COPY
//   - END
//   - WAIT_REMOVAL
//   - WAIT_REMOVAL_RESTART
//   - TOOLS
//   - WAIT_WIPE_KEY
//   - WIPE_KEY
//   - WAIT_IMAGE_KEY
//   - IMAGE_KEY
var state = "INIT";
var wipe_type = "quick";

// i18n - from https://www.webcodegeeks.com/html5/html5-internationalization-example/
var languages = Array.from(document.getElementsByClassName('language'));
var lang_http = new XMLHttpRequest();
var langDocument = {};

lang_http.onreadystatechange = function(){
    if (this.readyState === 4 && this.status === 200) {
        langDocument = JSON.parse(this.responseText);
        processLangDocument();
    }
};

function switchLanguage(language){
    lang_http.open("GET", "static/i18n/" + language + ".json", true);
    lang_http.setRequestHeader("Content-type", "application/json");
    lang_http.send();
}

function processLangDocument(){
    var tags = document.querySelectorAll('td,button,span,div,p,strong,img,a,label,li,option,h1,h2,h3,h4,h5,h6,small,summary,ul');
    Array.from(tags).forEach(function(value, index){
        var key = value.dataset.langkey;
        if(langDocument[key]) value.innerHTML = langDocument[key];
    });
}

function updateElementLang(element, langkey) {
  if(langDocument[langkey]) {
    element.setAttribute("data-langkey", langkey);
    element.innerHTML = langDocument[langkey];
  }
}

// Utils

var isSetsEqual = (a, b) => a.size === b.size && [...a].every((value) => b.has(value));

function fetchError(response) {
  // from https://www.tjvantoll.com/2015/09/13/fetch-and-errors
  if (!response.ok) {
    throw Error(response.statusText);
  }
  return response.body;
}

var parseHTML = function (str) {
  var tmp = document.implementation.createHTMLDocument();
  tmp.body.innerHTML = str;
  return tmp.body.children[0];
};

function set_state(new_state) {
  state = new_state;
  document.querySelector("body").setAttribute("data-state", new_state);
}

async function dispatchJSON(body, callback) {
  const reader = body.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    let data = String.fromCharCode.apply(null, value).split("\n");
    data.map((json_str) => {
      if (!json_str) return;
      try {
        callback(JSON.parse(json_str));
      } catch (error) {
        console.error("Error while parsing JSON");
        console.error(error);
      }
    });
  }
}

class Path {
  constructor(path, path_display, parent) {
    // Path ID
    this.path = path;
    // Path to display
    this.path_display = path_display;
    // Parent Path instance
    this.parent = parent;
  }
}

class File {
  constructor(path, size, ftype, timestamp) {
    // Path instance
    this.path = path;
    // Size in bytes
    this.size = size;
    /* ftype:
     * 1: regular file
     * 2: directory
     */
    this.ftype = ftype;
    this.timestamp = timestamp;
  }

  isDir() {
    return this.ftype == 2;
  }

  isFile() {
    return this.ftype == 1;
  }

  static humanFileSize(bytes) {
    // Inspired from http://stackoverflow.com/questions/10420352/converting-file-size-in-bytes-to-human-readable
    var thresh = 1024;
    if (Math.abs(bytes) < thresh) {
      return bytes + " o";
    }
    var units = ["Kio", "Mio", "Gio", "Tio", "Pio", "Eio", "Zio", "Yio"];
    var u = -1;
    do {
      bytes /= thresh;
      ++u;
    } while (Math.abs(bytes) >= thresh && u < units.length - 1);
    return bytes.toFixed(1) + " " + units[u];
  }

  date() {
    var a = new Date(this.timestamp * 1000);
    return (
      a.getDate() +
      "/" +
      parseInt(a.getMonth() + 1, 10) +
      "/" +
      a.getFullYear().toString().slice(-2) +
      " " +
      a.getHours() +
      ":" +
      a.getMinutes()
    );
  }

  get size_human() {
    return File.humanFileSize(this.size);
  }

  get path_display() {
    return this.path.path_display;
  }
}

class FileSystem {
  constructor() {
    this.file_paths = {};
    this.paths_info = {};
  }

  get_file(path_id) {
    return this.file_paths[path_id];
  }

  add_path(path) {
    // path: Path instance
    this.paths_info[path.path] = path;
  }

  add_file(file) {
    // file: File instance
    this.file_paths[file.path.path] = file;
    this.add_path(file.path);
  }

  get_path(path) {
    return this.paths_info[path];
  }
}

class Selection {
  constructor(fs) {
    // Set of normalized path, without redundancy between parent-directory and files
    // ('/a' implies '/a/b')
    this.selected = new Set();
    this.fs = fs;
  }

  delete(path) {
    this.selected.delete(path);
  }

  has_parent_selected(path) {
    let current = this.fs.get_path(path).parent;
    // Seek for parent directory
    while (current !== null) {
      if (this.selected.has(current.path)) {
        return true;
      }
      current = current.parent;
    }
    return false;
  }

  normalize_selected() {
    let modified = true;
    while (modified) {
      modified = false;
      for (let path of this.selected) {
        if (this.has_parent_selected(path)) {
          // 'path' already selected by a parent directory
          modified = true;
          this.selected.delete(path);
        }
      }
    }
  }

  add(path) {
    this.selected.add(path);
    this.normalize_selected();
  }

  has(path) {
    if (this.selected.has(path)) {
      return true;
    }
    return this.has_parent_selected(path);
  }

  *files() {
    let sorted = Array.from(this.selected).sort();
    for (let path of sorted) {
      yield this.fs.get_file(path);
    }
  }

  toJSON() {
    return {
      selected: Array.from(this.selected),
    };
  }
}

class Devices {
  constructor() {
    this.availables = [];
    this.device_in = undefined;
    this.device_out = undefined;
  }

  update_available(data) {
    let data_uniq = new Set(data.map((item) => item.id));
    let availables_uniq = new Set(this.availables.map((item) => item.id));
    if (isSetsEqual(data_uniq, availables_uniq)) {
      return false;
    }
    this.availables = data;
    return true;
  }

  static compare(device1, device2) {
    if (device1 === undefined || device2 === undefined) {
      return device1 === device2;
    }
    if (device1.id == device2.id) {
      return true;
    } else {
      return false;
    }
  }

  check_available() {
    let dev_in_found = false;
    let dev_out_found = false;
    for (let device of this.availables) {
      if (Devices.compare(device, this.device_in)) {
        dev_in_found = true;
      }
      if (Devices.compare(device, this.device_out)) {
        dev_out_found = true;
      }
    }
    if (!dev_in_found) {
      this.device_in = undefined;
    }
    if (!dev_out_found) {
      this.device_out = undefined;
    }
  }
}

function set_usbsas_infos(infos) {
  let usbsas_infos = document.getElementById("usbsas_infos");
  usbsas_infos.innerHTML = infos.name + infos.message;
  let version = document.getElementById("sasver");
  version.innerHTML = "Version: " + infos.version;
}

function get_usbsas_infos() {
  try {
    parse_json_and_call("usbsas_infos", set_usbsas_infos);
  } catch (error) {
    console.error(error);
  }
}

// Init
var fs = new FileSystem();
var cur_path = new Path(btoa(""), "/", null);
var selected = new Selection(fs);
var devices = new Devices();
var refresh_device;
var refresh_id;
var refresh_check_id;
var reset_timer;

function set_error(error_text) {
  error.classList.add("fadein100");
  error.classList.add("d-flex");
  error.innerText = error_text;
}

function clear_error() {
  error.classList.remove("fadein100");
  error.classList.remove("d-flex");
  error.innerText = "";
}

function throw_error(error_text) {
  console.error(error_text);
  set_error(error_text);
  throw "An error occurred";
}

function toggle_select(target) {
  // target: File instance
  let path = target.path.path;
  if (selected.has(path)) {
    selected.delete(path);
  } else {
    selected.add(path);
    fs.add_file(target);
  }
  update_selected_list();
}

function parse_json_and_call(url, callback) {
  var request = new XMLHttpRequest();
  request.open("GET", "/" + url, true);
  request.onload = function () {
    if (this.status >= 200 && this.status < 400) {
      var data = JSON.parse(this.response);
      callback(data);
    } else {
      throw_error(langDocument["err-fetch-url"] + " " + url);
    }
  };
  request.send();
}

function update() {
  var request = new XMLHttpRequest();
  request.open("GET", "/devices/dirty/read_dir/?path=" + cur_path.path, true);
  request.onload = function () {
    if (this.status >= 200 && this.status < 400) {
      var data = JSON.parse(this.response);
      var filelist = [];
      for (var file in data) {
        filelist.push(data[file]);
      }
      filelist.sort(function (a, b) {
        if (a.status > b.status) return -1;
        if (a.status < b.status) return 1;
        return a.path_display.localeCompare(b.path_display);
      });
      var items = [];
      document.querySelector("#listing").innerHTML = "";
      if (cur_path.parent !== null) {
        let tr = document.createElement("tr");

        let td = document.createElement("td");
        tr.appendChild(td);

        td = document.createElement("td");
        td.colspan = "2";

        let a = document.createElement("a");
        a.href = "#";

        a.onclick = function () {
          go(cur_path.parent);
        };
        let i = document.createElement("i");
        i.classList.add("fas");
        i.classList.add("fa-level-up-alt");
        i.innerText = "\u00a0";
        let strong = document.createElement("strong");
        strong.innerText = "[Parent directory]";

        a.appendChild(i);
        a.appendChild(strong);
        td.appendChild(a);
        tr.appendChild(td);

        document.querySelector("#listing").appendChild(tr);
      }

      document.querySelector("#path_explode").innerHTML = "";
      var current = cur_path;
      var parents_list = [];
      while (current !== null) {
        parents_list.push(current);
        current = current.parent;
      }
      current = parents_list.pop();
      parents_list.reverse();

      var last_path = "";

      var li = document.createElement("li");
      var a = document.createElement("a");
      a.href = "#";
      a.onclick = function () {
        go(current);
      };

      var i = document.createElement("i");
      i.classList.add("fas");
      i.classList.add("fa-home");

      a.appendChild(i);

      li.appendChild(a);
      document.querySelector("#path_explode").appendChild(li);

      parents_list.forEach(function (current) {
        var li = document.createElement("li");
        var a = document.createElement("a");
        a.href = "#";
        a.onclick = function () {
          go(current);
        };

        var strong = document.createElement("strong");
        strong.innerText = current.path_display;
        a.appendChild(strong);

        li.appendChild(a);
        document.querySelector("#path_explode").appendChild(li);

        last_path = current.path_display;
      });

      let tot_size = 0;
      let nb_folder = 0;
      let nb_files = 0;

      Array.prototype.forEach.call(Object.entries(filelist), function (obj, i) {
        let val = obj[1];
        let key = val.path;
        let path = new Path(val.path, val.path_display, cur_path);
        let file = new File(path, val.size, val.ftype, val.timestamp);
        fs.add_file(file);

        var cur_tr = document.createElement("tr");
        let th = document.createElement("th");
        th.classList.add("text-muted");
        var input = document.createElement("i");
        input.classList.add("far");

        let toggle_fn = function () {
          toggle_select(file);
          input.classList.remove("fa-square");
          input.classList.remove("fa-check-square");
          if (selected.has(key)) {
            input.classList.add("fa-check-square");
          } else {
            input.classList.add("fa-square");
          }
        };
        if (selected.has(key)) {
          input.classList.add("fa-check-square");
        } else {
          input.classList.add("fa-square");
        }
        input.onclick = toggle_fn;
        th.appendChild(input);


        cur_tr.style.cursor = "pointer";

        th.scope = "row";
        th.classList.add("col-1");
        cur_tr.classList.add("d-flex");
        cur_tr.appendChild(th);

        let td = document.createElement("td");
        var i = document.createElement("i");
        i.classList.add("fas");
        i.classList.add("fa-" + (file.isDir() ? "folder-open" : "file"));
        i.innerText = "\u00a0";
        td.appendChild(i);

        if (file.isDir()) {
          var a = document.createElement("a");
          a.href = "#";
          a.onclick = function () {
            go(new Path(key, file.path_display, cur_path));
          };
          td.appendChild(a);
          var strong = document.createElement("strong");
          strong.innerText = file.path_display.replace(cur_path.path_display, "").replace("/", "");
          a.appendChild(strong);
        } else {
          var span = document.createElement("span");
          span.innerText = file.path_display.replace(cur_path.path_display, "").replace("/", "");
          td.appendChild(span);
        }

        td.classList.add("col");
        td.classList.add("text-break");
        cur_tr.appendChild(td);

        td = document.createElement("td");
        td.setAttribute("data-sort-value", "0");
        td.classList.add("small");
        td.classList.add("col-2");

        td.innerText = file.date();
        cur_tr.appendChild(td);

        td = document.createElement("td");
        td.setAttribute("data-sort-value", "0");
        td.classList.add("small");
        td.classList.add("col-2");

        if (file.isDir()) {
          td.innerText = "-";
          nb_folder += 1;
        } else {
          td.innerText = file.size_human;
          tot_size += file.size;
          nb_files += 1;
        }

        cur_tr.appendChild(td);

        document.querySelector("#listing").appendChild(cur_tr);
      });

      let summary = "Total: " + File.humanFileSize(tot_size);
      document.querySelector("#summary").innerHTML = summary;

    } else {
      throw_error(langDocument["erreadpath"] + ": " + cur_path.path_display);
    }
  };

  request.error = function () {
    throw_error(langDocument["erreadpath"] + ": " + cur_path.path_display);
  };
  request.send();

  update_selected_list();
}

function update_selected_list() {
  document.querySelector("#selected-list").innerHTML = "";
  for (let file of selected.files()) {
    let li = document.createElement("li");
    li.classList.add("list-group-item");
    li.innerText = file.path_display;
    let icon = document.createElement("i");
    let span = document.createElement("span");
    span.style.float = "right";
    if (file.isDir()) {
      icon.classList.add("fas");
      icon.classList.add("fa-folder-open");
      span.innerHTML = "&mdash;";
    } else {
      icon.classList.add("fas");
      icon.classList.add("fa-file");
      span.innetText = file.size_human;
    }
    icon.innerHTML = "&nbsp;";
    li.insertBefore(icon, li.firstChild);
    li.appendChild(span);

    document.querySelector("#selected-list").appendChild(li);
  }
  if (selected.selected.size == 0) {
    document.querySelector("#launch-button").setAttribute("disabled", "disabled");
  } else {
    document.querySelector("#launch-button").removeAttribute("disabled");
  }
}

function select_all() {
  let sel_all = document.querySelector("#view-choice table th:first-child i");
  let checkboxes = document.querySelectorAll("#listing th");
  if (sel_all.classList.contains("fa-square")) {
    sel_all.classList.remove("fa-square");
    sel_all.classList.add("fa-check-square");
    for (var item of checkboxes) {
      if (item.firstChild.classList.contains("fa-square")) item.firstChild.click();
    }
  } else {
    sel_all.classList.remove("fa-check-square");
    sel_all.classList.add("fa-square");
    for (var item of checkboxes) {
      if (item.firstChild.classList.contains("fa-check-square")) item.firstChild.click();
    }
  }
}

function go(target_path) {
  cur_path = target_path;
  update();
}


function do_copy() {
  set_state("COPY");
  document.querySelector("#copy-options").classList.add("d-none");
  let tbody = document.querySelector("#copy table tbody");
  let progress = document.querySelector("#copy div.progress-bar");

  let elements = [];
  let add_status = function (json) {
    let name = json.status;

    if (langDocument.hasOwnProperty(name)) {
      if (elements.length > 0) {
        elements[elements.length - 1].icon.classList.remove("spinner-border");
        elements[elements.length - 1].icon.classList.add("fa-check");
      }

      let tr = document.createElement("tr");
      let status_td = document.createElement("td");
      status_td.innerHTML = "&nbsp;";
      let status_icon = document.createElement("i");
      status_icon.classList.add("fas");
      for (var icon_class of ["spinner-border", "spinner-border-sm"]) {
        status_icon.classList.add(icon_class);
      }
      status_td.insertBefore(status_icon, status_td.firstChild);
      tr.appendChild(status_td);
      let name_td = document.createElement("td");
      let sp = document.createElement("span");
      name_td.appendChild(sp);
      updateElementLang(name_td, name);
      tr.appendChild(name_td);
      elements.push({
        icon: status_icon,
        tr: tr,
        name: name_td,
      });

      tbody.appendChild(elements[elements.length - 1].tr);
    }

    if (json.hasOwnProperty("progress")) {
      let progress_value = Number(json.progress).toFixed(2);
      progress.style.width = progress_value + "%";
      progress.innerText = progress_value + "%";
    }
  };

  let percent;
  let has_error = false;
  let reset_timeout = 1000;
  let update_display = function (json) {
    // console.log(JSON.stringify(json, null, 2));

      switch (json.status) {
        case "copy_not_enough_space":
          has_error = true;
          reset_timeout = 7000;
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-times");
          let tr = document.createElement("tr");
          tr.innerHTML =
            "<td><i class='fas fa-times'></i>&nbsp;</td><td><strong data-langkey=\"devicetoosmall\">" +
            langDocument["devicetoosmall"] +
            "</strong> (" + File.humanFileSize(json.size) +")</td>";
          tbody.appendChild(tr);
          progress.classList.add("bg-danger");
          break;
        case "nothing_to_copy":
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-check");
          progress.classList.add("bg-danger");
          let nothing_tr = document.createElement("tr");
          nothing_tr.innerHTML =
            "<td><i class='fas fa-times'></i>&nbsp;</td><td><strong data-langkey=\"nothingtocopy\">" +
            langDocument["nothingtocopy"] + "</strong></td>";
          tbody.appendChild(nothing_tr);
          document.querySelector("#cancel-button").classList.remove("d-none");
          document.querySelector("#cancel-button").innerText = langDocument["return"];
          for (let filtered_path of json.filtered_path) {
            // Display filtered elements
            has_error = true;
            let tr_err = document.createElement("tr");
            let status_td_err = document.createElement("td");
            status_td_err.innerHTML = "&nbsp;";
            let format_status_icon_err = document.createElement("i");
            format_status_icon_err.classList.add("fas");
            format_status_icon_err.classList.add("fa-times");
            format_status_icon_err.classList.add("text-danger");
            status_td_err.insertBefore(format_status_icon_err, status_td_err.firstChild);
            tr_err.appendChild(status_td_err);
            let name_td_err = document.createElement("td");
            let p_err = document.createElement("p");
            p_err.classList.add("text-danger");
            p_err.innerHTML = "<strong data-langkey=\"filterf\">" + langDocument["filterf"] + "</strong>";
            let span_err = document.createElement("span");
            span_err.innerText = filtered_path;
            p_err.appendChild(span_err);
            name_td_err.appendChild(p_err);
            tr_err.appendChild(name_td_err);
            tbody.appendChild(tr_err);
          }
          for (let dirty_path of json.dirty_path) {
            // Display dirty elements
            has_error = true;
            let tr_err = document.createElement("tr");
            let status_td_err = document.createElement("td");
            status_td_err.innerHTML = "&nbsp;";
            let format_status_icon_err = document.createElement("i");
            format_status_icon_err.classList.add("fas");
            format_status_icon_err.classList.add("fa-times");
            format_status_icon_err.classList.add("text-danger");
            status_td_err.insertBefore(format_status_icon_err, status_td_err.firstChild);
            tr_err.appendChild(status_td_err);
            let name_td_err = document.createElement("td");
            let p_err = document.createElement("p");
            p_err.classList.add("text-danger");
            p_err.innerHTML = "<strong data-langkey=\"filterav\">" + langDocument["filterav"] + "</strong>";
            let span_err = document.createElement("span");
            span_err.innerText = dirty_path;
            p_err.appendChild(span_err);
            name_td_err.appendChild(p_err);
            tr_err.appendChild(name_td_err);
            tbody.appendChild(tr_err);
          }
          set_state("WAIT_REMOVAL");
          break;
        case "terminate":
          // Effective write on device
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-check");
          let terminate_tr = document.createElement("tr");
          terminate_tr.innerHTML =
            "<td><i class='fas fa-save'></i>&nbsp;</td><td><strong data-langkey=\"transferdone\">" +
            langDocument["transferdone"] + "</strong></td>";
          tbody.appendChild(terminate_tr);
          progress.style.width = "100%";
          progress.innerText = "100%";
          progress.classList.add("bg-success");
          document.querySelector("#cancel-button").removeAttribute("disabled");
          set_state("WAIT_REMOVAL");
          if (has_error) {
            reset_timeout = 7000;
          }
          document.querySelector("#cancel-button").classList.remove("d-none");
          document.querySelector("#cancel-button").innerText = langDocument["return"];
          break;
        case "final_report":
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-check");
          for (let error_path of json.error_path) {
            // Display failed elements
            has_error = true;
            let tr_err = document.createElement("tr");
            let status_td_err = document.createElement("td");
            status_td_err.innerHTML = "&nbsp;";
            let format_status_icon_err = document.createElement("i");
            format_status_icon_err.classList.add("fas");
            format_status_icon_err.classList.add("fa-times");
            format_status_icon_err.classList.add("text-danger");
            status_td_err.insertBefore(format_status_icon_err, status_td_err.firstChild);
            tr_err.appendChild(status_td_err);

            let name_td_err = document.createElement("td");
            let p_err = document.createElement("p");
            p_err.classList.add("text-danger");
            p_err.innerHTML = "<strong data-langkey=\"copyerr\">" + + langDocument["copyerr"] + "</strong>";
            let span_err = document.createElement("span");
            span_err.innerText = error_path;
            p_err.appendChild(span_err);
            name_td_err.appendChild(p_err);
            tr_err.appendChild(name_td_err);
            tbody.appendChild(tr_err);
          }
          for (let filtered_path of json.filtered_path) {
            // Display failed elements
            has_error = true;
            let tr_err = document.createElement("tr");
            let status_td_err = document.createElement("td");
            status_td_err.innerHTML = "&nbsp;";
            let format_status_icon_err = document.createElement("i");
            format_status_icon_err.classList.add("fas");
            format_status_icon_err.classList.add("fa-times");
            format_status_icon_err.classList.add("text-danger");
            status_td_err.insertBefore(format_status_icon_err, status_td_err.firstChild);
            tr_err.appendChild(status_td_err);

            let name_td_err = document.createElement("td");
            let p_err = document.createElement("p");
            p_err.classList.add("text-danger");
            p_err.innerHTML = "<strong data-langkey=\"filterf\">" + langDocument["filterf"] + "</strong>";
            let span_err = document.createElement("span");
            span_err.innerText = filtered_path;
            p_err.appendChild(span_err);
            name_td_err.appendChild(p_err);
            tr_err.appendChild(name_td_err);
            tbody.appendChild(tr_err);
          }
          for (let dirty_path of json.dirty_path) {
            // Display failed elements
            has_error = true;
            let tr_err = document.createElement("tr");
            let status_td_err = document.createElement("td");
            status_td_err.innerHTML = "&nbsp;";
            let format_status_icon_err = document.createElement("i");
            format_status_icon_err.classList.add("fas");
            format_status_icon_err.classList.add("fa-times");
            format_status_icon_err.classList.add("text-danger");
            status_td_err.insertBefore(format_status_icon_err, status_td_err.firstChild);
            tr_err.appendChild(status_td_err);

            let name_td_err = document.createElement("td");
            let p_err = document.createElement("p");
            p_err.classList.add("text-danger");
            p_err.innerHTML = "<strong data-langkey=\"filterav\">" + langDocument["filterav"] + "</strong>";
            let span_err = document.createElement("span");
            span_err.innerText = dirty_path;
            p_err.appendChild(span_err);
            name_td_err.appendChild(p_err);
            tr_err.appendChild(name_td_err);
            tbody.appendChild(tr_err);
          }
          break;
        case "fatal_error":
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-times");
          progress.classList.add("bg-danger");
          reset_timeout = 7000;
          document.querySelector("#cancel-button").classList.remove("d-none");
          document.querySelector("#cancel-button").removeAttribute("disabled");
          document.querySelector("#cancel-button").innerText = "Retour";
          let fatal_error = document.createElement("tr");
          fatal_error.innerHTML =
            "<td><i class='fas fa-times'></i>&nbsp;</td><td><strong data-langkey=\"trfail\">" +
            langDocument["trfail"] + "</strong><q id='error-reason'></q></td>";
          fatal_error.classList.add("text-danger");
          tbody.appendChild(fatal_error);
          document.querySelector("#error-reason").innerText = json.msg;
          break;
        case "cmd_error":
          has_error = true;
          elements[elements.length - 1].icon.classList.remove("spinner-border");
          elements[elements.length - 1].icon.classList.add("fa-times");
          let error_cmd_tr = document.createElement("tr");
          error_cmd_tr.innerHTML =
            "<td><i class='fas fa-times'></i>&nbsp;</td><td><strong data-langkey=\"trfail\">" +
            langDocument["trfail"] + "</strong><q id='cmd-error-reason'></q></td>";
          tbody.appendChild(error_cmd_tr);
          document.querySelector("#cmd-error-reason").innerText = json.result;
          progress.classList.add("bg-danger");
          document.querySelector("#cancel-button").removeAttribute("disabled");
          set_state("WAIT_REMOVAL");
          if (has_error) {
            reset_timeout = 7000;
            document.querySelector("#cancel-button").classList.remove("d-none");
            document.querySelector("#cancel-button").innerText = langDocument["return"];
          }
          break;
        default:
          add_status(json);
      }
  };

  var fsfmt = document.querySelector("#fsfmt");
  var post_body = selected.toJSON();
  post_body.fsfmt = fsfmt.options[fsfmt.selectedIndex].value;

  fetch("/copy", {
    method: "POST",
    body: JSON.stringify(post_body),
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
  })
    .then(fetchError)
    .then((body) => dispatchJSON(body, update_display))
    .catch((error) => {
      console.error(error);
      document.querySelector("#cancel-button").removeAttribute("disabled");
      throw_error(langDocument["cpsrverror"]);
    });
}

function select_partition(partition) {
  document.querySelector("#partition-choice").style.display = "none";
  var request = new XMLHttpRequest();
  request.open("GET", "/devices/dirty/open/" + partition.index, true);
  request.onload = function () {
    if (this.status >= 200 && this.status < 400) {
      set_state("SELECT_FILES");
      var viewChoice = document.querySelector("#view-choice");
      viewChoice.style.display = "block";
      viewChoice.classList.add("show");
      go(cur_path);
    } else {
      document.querySelector("#partition-choice").style.visibility = "hidden";
      if (this.responseText == "\"ntfs volume is dirty\"") {
        document.querySelector("#parterror").innerText = langDocument["ntfsdirty"];
        document.querySelector("#parterror").classList.remove('d-none');
        throw_error(langDocument["erropenpart"]);
      } else {
        throw_error(langDocument["erropenpart"]);
      }
    }
  };
  request.error = function () {
    throw_error(langDocument["errgetdev"]);
  };
  request.send();
}

function partition_choice() {
  var part_table = document.querySelector("#partition-view");
  var request = new XMLHttpRequest();
  request.open("GET", "/devices/dirty", true);
  request.onload = function () {
    if (this.status >= 200 && this.status < 400) {
      var data = JSON.parse(this.response);
      // Sort by start
      data.sort(function (a, b) {
        if (a["start"] < b["start"]) return -1;
        if (a["start"] > b["start"]) return 1;
        throw "Same start, error";
      });
      if (data.length == 1) {
        select_partition(data[0]);
        return;
      }
      Array.prototype.forEach.call(data, function (partition, i) {
        let row = document.createElement('tr');
        row.classList.add("m-0");
        row.classList.add("d-flex");

        let cell = document.createElement('td');
        let display = document.createElement("strong");
        display.innerText = partition.name_str;
        cell.classList.add("col");
        cell.appendChild(display);
        row.appendChild(cell);

        cell = document.createElement('td');
        cell.innerText = File.humanFileSize(partition.size);
        cell.classList.add("col-2");
        row.appendChild(cell);

        cell = document.createElement('td');
        cell.classList.add("col-2");
        row.appendChild(cell);
        if ( partition.ptype == 0 ) {
          cell.innerText = "[unsupported]";
          row.classList.add("table-danger");
          row.style.pointerEvents = "none";
        } else {
          cell.innerText = partition.type_str;
          row.onclick = function() {
            select_partition(partition);
          };
        };
        document.querySelector("#partition-view").appendChild(row);
      });
    } else {
      throw_error(langDocument["errfs"]);
    }
  };
  request.error = function () {
    throw_error(langDocument["errgetdev"]);
  };
  request.send();
}

function render_device_choice() {
  document.querySelector("#device-in").innerHTML = "";
  document.querySelector("#device-out").innerHTML = "";
  for (let device of devices.availables) {
    for (let type of ["in", "out"]) {
      let a = document.createElement("a");
      a.classList.add("list-group-item");
      a.classList.add("list-group-item-action");
      a.href = "#";
      if (type == "in" && device.is_src) {
        if (state == "WAIT_SOURCE") {
          devices.device_in = device;
          set_state("WAIT_DESTINATION");
          updateElementLang(document.querySelector("#usb-arrow p"), "insert-dest");
          document.querySelector("#usb-arrow img").src = "static/img/device_out.svg";
        }
        /*
        a.onclick = function () {
          if (a.classList.contains("disabled")) {
            return false;
          } else if (Devices.compare(devices.device_in, device)) {
            devices.device_in = undefined;
          } else {
            devices.device_in = device;
          }
          check_render_device_choice();
          return false;
        };
        */
      } else if (type == "out" && device.is_dst) {
        if (
          devices.device_out == undefined &&
          device.dev_type == "Usb" &&
          !Devices.compare(devices.device_in, device)
        ) {
          devices.device_out = device;
          updateElementLang(document.querySelector("#removal h4"), "rmdev");
          check_render_device_choice();
        }
        a.onclick = function () {
          if (a.classList.contains("disabled")) {
            return false;
          } else if (Devices.compare(devices.device_out, device)) {
            devices.device_out = undefined;
          } else {
            devices.device_out = device;
          }
          check_render_device_choice();
          return false;
        };
      }

      if (!((type == "in" && device.is_src) || (type == "out" && device.is_dst))) {
        continue;
      }
      let p = document.createElement("p");
      p.classList.add("list-group-item-text");
      let h4 = document.createElement("h4");
      h4.classList.add("list-group-item-heading");
      if (type == "out" && Devices.compare(devices.device_in, device)) {
        updateElementLang(h4, "usb-dev");
        updateElementLang(p, "usb-dest-descr");
      } else if (device.dev_type == "Usb") {
        h4.innerText = device.dev.Usb.description;
        p.innerText = device.dev.Usb.manufacturer + ", serial " + device.dev.Usb.serial;
      } else if (device.dev_type == "Net") {
        p.innerText = device.dev.Net.longdescr;
        h4.innerText = device.dev.Net.description;
      } else if (device.dev_type == "Cmd") {
        p.innerText = device.dev.Cmd.longdescr;
        h4.innerText = device.dev.Cmd.description;
      }

      a.appendChild(h4);
      a.appendChild(p);
      if (type == "in") {
        document.querySelector("#device-in").appendChild(a);
        if (Devices.compare(device, devices.device_in)) {
          a.classList.add("active");
        } else if (Devices.compare(device, devices.device_out)) {
          a.classList.add("disabled");
        }
      } else if (type == "out") {
        document.querySelector("#device-out").appendChild(a);
        if (Devices.compare(device, devices.device_out)) {
          a.classList.add("active");
        } else if (Devices.compare(device, devices.device_in)) {
          a.classList.add("disabled");
        }
      }
    }
  }
}

function check_render_device_choice() {
  devices.check_available();
  if (devices.device_in !== undefined && devices.device_out !== undefined) {
    clearInterval(refresh_device);
    var request = new XMLHttpRequest();
    request.open(
      "GET", "/devices/select/" + devices.device_in.id + "/" + devices.device_out.id,
      true
    );
    let destination_descr = "";
    if (devices.device_out.dev_type == "Usb") {
      document.querySelector("#copy-options").classList.remove('d-none');
      destination_descr = langDocument["outusb"];
    } else if (devices.device_out.dev_type == "Net") {
      destination_descr = devices.device_out.dev.Net.description;
    } else if (devices.device_out.dev_type == "Cmd") {
      destination_descr = devices.device_out.dev.Cmd.description;
    }
    document.querySelector("#destination-descr").innerText = destination_descr;
    request.onload = function () {
      if (this.status >= 200 && this.status < 400) {
        set_state("SELECT_PARTITION");
        partition_choice();
      } else {
        devices.device_in = undefined;
        devices.device_out = undefined;
        throw_error(langDocument["errseldev"] + ": " + this.response);
      }
      return;
    };
    request.error = function () {
      throw_error(langDocument["errseldev"] + ": " + this.response);
    };
    request.send();
  }
  render_device_choice();
}

function restart() {
  set_state("WAIT_REMOVAL_RESTART");
  check_device_removal();
  reset_usbsas();
}

function check_device_removal() {
  let usb_count = 0;
  let src_plugged = false;
  for (let device of devices.availables) {
    if (device.dev_type == "Usb") {
      usb_count += 1;
      if (devices.device_in && Devices.compare(device, devices.device_in)) src_plugged = true;
    }
  }
  if (usb_count == 0 || (!state.startsWith("WAIT_REMOVAL") && !src_plugged)) {
    document.location.reload(true);
  }
}

function device_choice() {
  var request = new XMLHttpRequest();
  request.open("GET", "/devices", true);
  request.onload = function () {
    clear_error();
    if (state == "INIT") {
      set_state("WAIT_SOURCE");
    }
    if (this.status >= 200 && this.status < 400) {
      var data = JSON.parse(this.response);
      if (devices.update_available(data)) {
        if (state == "WAIT_REMOVAL" || state == "WAIT_REMOVAL_RESTART") {
          check_device_removal();
        } else if (state == "WAIT_WIPE_KEY" || state == 'WAIT_IMAGE_KEY') {
          confirm_key();
        } else {
          if (state == "WAIT_DESTINATION") check_device_removal();
          check_render_device_choice();
        }
      }
    } else {
      throw_error(langDocument["errgetdev"]);
    }
  };
  request.error = function () {
    throw_error(langDocument["errgetdev"]);
  };
  request.onerror = function () {
    document.location.reload(true);
  };
  request.send();
}

function check_id() {
  var request = new XMLHttpRequest();
  request.open("GET", "/id", true);
  request.onload = function () {
    clear_error();
    if (this.status >= 400) {
      throw_error(langDocument["errgetid"]);
    }
  };
  request.error = function () {
    throw_error(langDocument["errgetid"]);
  };
  request.onerror = function () {
    document.location.reload(true);
  };
  request.send();
}

function get_id() {
  var request = new XMLHttpRequest();
  request.open("GET", "/id", true);
  request.onload = function () {
    if (this.status >= 200 && this.status < 400) {
      var data = JSON.parse(this.response);
      if (data.length > 0) {
        document.querySelector("#id-num div").innerHTML = "<h4 id='id-content'></h4>";
        document.querySelector("#id-content").innerText = "ID: " + data;
        document.querySelector("#id-num div").classList.add("alert-success");
        document.querySelector("#id-num div").classList.remove("alert-info");
        document.querySelector("#id-num").style.height = "60px";

        clearInterval(refresh_id);
        clearInterval(refresh_check_id);
        do_copy();
      }
    } else {
      throw_error(langDocument["errgetid"]);
    }
  };
  request.error = function () {
    throw_error(langDocument["errgetid"]);
  };
  request.send();
}

function do_id_and_copy() {
  set_state("WAIT_ID");
  get_id();
  refresh_id = setInterval(get_id, 1000);
}

function tool_device_choice(action) {
  if (action == 'wipe') {
    set_state("WAIT_WIPE_KEY");
    document.querySelector("#copy-options").classList.remove('d-none');
    document.querySelector("#wipe-warning").classList.remove('d-none');
    updateElementLang(document.querySelector("#usb-arrow p"), "insertwipe");
  } else if (action == 'imagedisk') {
    set_state("WAIT_IMAGE_KEY");
    updateElementLang(document.querySelector("#usb-arrow p"), "insertimg");
  }
  document.querySelector("#cancel-button").innerText = langDocument["cancel"];
  document.querySelector("#usb-arrow img").src = "static/img/device_wipe.svg";
  device_choice();
}

function confirm_key() {
  for (let device of devices.availables) {
    if (device.is_src) {
      devices.device_in = device;
    }
  }
  document.querySelector("#key-manuf").innerText = devices.device_in.dev.Usb.manufacturer
    + " " + devices.device_in.dev.Usb.description;
  document.querySelector("#key-serial").innerText = devices.device_in.dev.Usb.serial;
  document.querySelector("#usb-arrow").style.visibility = "hidden";
  document.querySelector("#confirm-button").removeAttribute("disabled");
}

function do_tool() {
  if (state == 'WAIT_WIPE_KEY') {
    do_wipe_key();
  } else if (state == 'WAIT_IMAGE_KEY') {
    do_image_disk();
  }
}

function do_wipe_key() {
  set_state("WIPE_KEY");
  clearInterval(refresh_device);
  clearInterval(refresh_id);
  clearInterval(refresh_check_id);
  document.querySelector("#copy-options").classList.add("d-none");
  let wipe_message = document.querySelector("#tool-message");
  updateElementLang(wipe_message, "formatting");
  let wipe_icon = document.querySelector("#tool-icon");
  let wipe_time = document.querySelector("#tool-time");
  let wipe_time_value = document.querySelector("#tool-time-value");
  let progress = document.querySelector("#tool-page div.progress-bar");
  let percent = 0;
  progress.innerText = percent + "%";
  progress.style.width = percent + "%";

  let reset_timeout = 1000;
  let time_start = new Date().getTime();
  let time_now = 0;
  let update_display = function (data) {
    // console.log(JSON.stringify(data, null, 2));
    switch (data.status) {
      case "wipe_start":
        percent = 1;
        progress.style.width = percent + "%";
        progress.innerText = percent + "%";
        break;
      case "wipe_status":
        if (data.current_size != 0 && data.total_size != 0) {
          time_now = new Date().getTime();
          let minutes = ~~(
            (time_now - time_start) * (data.total_size - data.current_size)
            / data.current_size / 1000 / 60);
          if (minutes < 1) {
            wipe_time_value.innerText = "<1";
          } else {
            wipe_time_value.innerText = minutes;
          }
          wipe_time.removeAttribute("hidden");
          percent = 1 + (90 * data.current_size) / data.total_size;
          percent = percent.toFixed(2);
          progress.style.width = percent + "%";
          progress.innerText = percent + "%";
        }
        break;
      case "format_status":
        if (data.current_size != 0 && data.total_size != 0) {
          percent = 91 + (8 * data.current_size) / data.total_size;
          percent = percent.toFixed(2);
          progress.style.width = percent + "%";
          progress.innerText = percent + "%";
        }
        break;
      case "wipe_end":
        updateElementLang(wipe_message, "formatend");
        percent = 100;
        wipe_time.setAttribute("hidden", true);
        progress.innerText = percent + "%";
        progress.style.width = percent + "%";
        progress.classList.remove("progress-bar-striped");
        progress.classList.remove("progress-bar-animated");
        progress.classList.remove("bg-info");
        progress.classList.add("bg-success");
        wipe_icon.classList.remove("spinner-border");
        wipe_icon.classList.remove("spinner-border-sm");
        wipe_icon.classList.add("fa-check");
        document.querySelector("#cancel-button").innerText = langDocument["return"];
        break;
      case "error":
      case "fatal_error":
        progress.classList.remove("bg-info");
        progress.classList.add("bg-danger");
        wipe_icon.classList.remove("spinner-border");
        wipe_icon.classList.remove("spinner-border-sm");
        wipe_icon.classList.add("fa-times");
        throw_error(langDocument["formaterr"]);
        document.querySelector("#cancel-button").innerText = "Retour";
        break;
      default:
        console.error("Unknown status message: ", json);
    }
  };

  var quick = false;
  if (wipe_type == "quick") {
    quick = true;
  }

  var fschoice = document.querySelector("#fsfmt");
  var fsfmt = fschoice.options[fschoice.selectedIndex].value;

  fetch("/wipe" + "/" + devices.device_in.id + "/" + fsfmt + "/" + quick, {
    method: "GET",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
  })
    .then(fetchError)
    .then((body) => dispatchJSON(body, update_display))
    .catch((error) => {
      console.error(error);
      document.querySelector("#cancel-button").removeAttribute("disabled");
      throw_error(langDocument["formaterr"]);
    });
  clearInterval(refresh_device);
}

function do_image_disk() {
  set_state("IMAGE_KEY");
  clearInterval(refresh_device);
  clearInterval(refresh_id);
  clearInterval(refresh_check_id);

  let message = document.querySelector("#tool-message");
  updateElementLang(message, "imaging");
  let icon = document.querySelector("#tool-icon");
  let time = document.querySelector("#tool-time");
  let time_value = document.querySelector("#tool-time-value");
  let progress = document.querySelector("#tool-page div.progress-bar");
  let percent = 0;
  progress.innerText = percent + "%";
  progress.style.width = percent + "%";

  let reset_timeout = 1000;
  let time_start = new Date().getTime();
  let time_now = 0;
  let total_size = devices.device_in.size;

  let update_display = function(data) {
    // console.log(JSON.stringify(data, null, 2));
    switch (data.status) {
      case "imgdisk_start":
        percent = 1;
        progress.style.width = percent + "%";
        progress.innerText = percent + "%";
        break;
      case "imgdisk_update":
        if (data.current_size != 0 && data.total_size != 0) {
          time_now = new Date().getTime();
          let minutes = ~~(
            (time_now - time_start) * (data.total_size - data.current_size)
            / data.current_size / 1000 / 60);
          if (minutes < 1) {
            time_value.innerText = "<1";
          } else {
            time_value.innerText = minutes;
          }
          time.removeAttribute("hidden");
          percent = 1 + (90 * data.current_size) / data.total_size;
          percent = percent.toFixed(2);
          progress.style.width = percent + "%";
          progress.innerText = percent + "%";
        }
        break;
      case "imgdisk_end":
        updateElementLang(message, "imgend");
        percent = 100;
        time.setAttribute("hidden", true);
        progress.innerText = percent + "%";
        progress.style.width = percent + "%";
        progress.classList.remove("progress-bar-striped");
        progress.classList.remove("progress-bar-animated");
        progress.classList.remove("bg-info");
        progress.classList.add("bg-success");
        icon.classList.remove("spinner-border");
        icon.classList.remove("spinner-border-sm");
        icon.classList.add("fa-check");
        document.querySelector("#cancel-button").innerText = langDocument["return"];
        break;
      case "error":
        progress.classList.remove("bg-info");
        progress.classList.add("bg-danger");
        icon.classList.remove("spinner-border");
        icon.classList.remove("spinner-border-sm");
        icon.classList.add("fa-times");
        throw_error(langDocument["imgerr"]);
        document.querySelector("#cancel-button").innerText = langDocument["return"];
        break;
      default:
        console.error("Unknown status message: ", json);
    }
  };

  fetch("/imagedisk" + "/" + devices.device_in.id, {
    method: "GET",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
  })
    .then(fetchError)
    .then((body) => dispatchJSON(body, update_display))
    .catch((error) => {
      console.error(error);
      document.querySelector("#cancel-button").removeAttribute("disabled");
      throw_error(langDocument["imgerr"]);
    });
  clearInterval(refresh_device);

}

function reload_document() {
  document.location.reload(true);
}

function reset_usbsas() {
  get_usbsas_infos();
  clearInterval(reset_timer);
  var request = new XMLHttpRequest();
  request.open("GET", "/reset", true);
  request.onload = function (data) {
    if (this.status >= 200 && this.status < 400) {
      device_choice();
      refresh_device = setInterval(device_choice, 1000);
      refresh_check_id = setInterval(check_id, 2000);
    } else {
      throw_error(langDocument["reseterr"]);
    }
  };
  request.error = function () {
    throw_error(langDocument["reseterr"]);
  };
  request.onerror = function () {
    set_error(langDocument["errconnsrv"]);
    reset_timer = setInterval(reload_document, 1000);
  };

  request.send();
}

document.addEventListener("readystatechange", (event) => {
  if (document.readyState == "interactive") {
    reset_usbsas();
  }
  document.querySelector("#schema").addEventListener("mouseenter", function () {
    document.querySelector("#schema").classList.add("anim_usb");
  });
  document.querySelector("#schema").addEventListener("mouseleave", function () {
    document.querySelector("#schema").classList.remove("anim_usb");
  });
});

function add_td_to_tr(text, tr) {
  let td = document.createElement("td");
  td.colspan = "1";
  td.style.padding = "0px";
  let elem = document.createElement("p");
  elem.style.marginBottom = "0px";
  elem.innerText = text;
  td.appendChild(elem);
  tr.appendChild(td);
}

function display_infos(data) {
  let table_mount = document.querySelector("#table-mount");
  table_mount.textContent = "";
  for (var item in data.mount) {
    let obj = data.mount[item];
    if (
      obj.fs_type != "ext4" &&
      obj.fs_type != "tmpfs" &&
      obj.fs_type != "xfs" &&
      obj.fs_type != "vfat"
    ) {
      continue;
    }

    let tr = document.createElement("tr");

    add_td_to_tr(obj.fs_mounted_from, tr);
    add_td_to_tr(obj.fs_type, tr);
    add_td_to_tr(obj.fs_mounted_on, tr);
    add_td_to_tr(obj.size_avail, tr);
    add_td_to_tr(obj.size_total, tr);

    table_mount.appendChild(tr);
  }

  let table_net = document.querySelector("#table-net");
  table_net.textContent = "";
  for (var item in data.network) {
    let obj = data.network[item];

    let tr = document.createElement("tr");

    add_td_to_tr(obj.name, tr);
    add_td_to_tr(obj.addrs, tr);
    add_td_to_tr(obj.mac, tr);

    table_net.appendChild(tr);
  }

  let memory = document.querySelector("#memory");
  memory.textContent = data.memory.used + " / " + data.memory.total;

  let load = document.querySelector("#load");
  load.textContent = data.load.one + " / " + data.load.five + " / " + data.load.fifteen;

  let time = document.querySelector("#time");
  time.textContent = data.time;
}

function updateFsFmtDetails() {
  let details = document.querySelector("#fsfmt-details");
  let select = document.querySelector("#fsfmt");
  switch(select.options[select.selectedIndex].value) {
    case 'ntfs':
      details.innerHTML = "";
      break;
    case 'exfat':
      details.innerHTML = "";
      break;
    case 'fat32':
      updateElementLang(details, "warng4gb");
      break;
  }
}

var ready = (callback) => {
  if (document.readyState != "loading") callback();
  else document.addEventListener("DOMContentLoaded", callback);
};

ready(() => {
  document.querySelector("#footer").addEventListener("dblclick", function (e) {
    try {
      parse_json_and_call("server_infos", display_infos);
      document.querySelector("#modalInfos").style.display = "block";
      document.querySelector("#modalInfos").className = "modal fade show";
    } catch (error) {
      console.error(error);
    }
  });

  document.querySelector("#modal-close-btn").addEventListener("click", (e) => {
    document.querySelector("#modalInfos").className = "modal fade";
    document.querySelector("#modalInfos").style.display = "none";
  });
});
