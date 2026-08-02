import "bootstrap/dist/css/bootstrap.min.css";
import { createApp } from "vue";
import App from "./App.vue";
import { router } from "./router";
import { setAuthToken } from "./generated/api_client";
import "./admin/admin.css";

const token = window.localStorage.getItem("che_rest_admin_token");
if (token) {
  setAuthToken(token);
}

createApp(App).use(router).mount("#app");
