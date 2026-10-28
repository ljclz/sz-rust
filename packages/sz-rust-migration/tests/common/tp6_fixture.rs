// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::PathBuf;
use tempfile::TempDir;

pub struct Tp6Fixture {
    _temp: TempDir,
    root: PathBuf,
}

impl Tp6Fixture {
    pub async fn create_minimal() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();

        let composer = r#"{"require": {"topthink/framework": "^6.0.13"}}"#;
        tokio::fs::write(root.join("composer.json"), composer)
            .await
            .unwrap();

        tokio::fs::create_dir_all(root.join("route")).await.unwrap();
        let route_content = r#"<?php
use think\facade\Route;
Route::get('user/info', [UserController::class, 'info']);
Route::post('user/create', [UserController::class, 'create']);
"#;
        tokio::fs::write(root.join("route").join("user.php"), route_content)
            .await
            .unwrap();

        tokio::fs::create_dir_all(root.join("app").join("model"))
            .await
            .unwrap();
        let model_content = r#"<?php
namespace app\model;
use think\Model;
class User extends Model {
    protected $table = 'users';
    public function profile() {
        return $this->hasOne(Profile::class, 'user_id', 'id');
    }
}
"#;
        tokio::fs::write(
            root.join("app").join("model").join("User.php"),
            model_content,
        )
        .await
        .unwrap();

        tokio::fs::create_dir_all(root.join("app").join("controller"))
            .await
            .unwrap();
        let controller_content = r#"<?php
namespace app\controller;
use think\Controller;
class UserController extends Controller {
    public function info($id) {
        return 'user info';
    }
    public function create($name, $email = '') {
        return 'created';
    }
}
"#;
        tokio::fs::write(
            root.join("app")
                .join("controller")
                .join("UserController.php"),
            controller_content,
        )
        .await
        .unwrap();

        tokio::fs::create_dir_all(root.join("app").join("middleware"))
            .await
            .unwrap();

        Self { _temp: temp, root }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}
