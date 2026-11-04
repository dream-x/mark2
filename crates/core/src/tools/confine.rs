//! Замок путей тулов внутри рабочей директории — общий для `read` и `grep`.
//!
//! `ToolCtx::resolve` только склеивает путь с cwd: абсолютный путь, `..` или
//! симлинк выводят из него куда угодно. Здесь граница проверяется по существу:
//!
//! * **Канонизируются обе стороны.** И cwd, и запрошенный путь проходят через
//!   `canonicalize` (`..` и симлинки раскрыты), и результат обязан лежать
//!   внутри канонизированного cwd. `Path::starts_with` сравнивает компоненты,
//!   так что `/work-other` за `/work` не сойдёт. Канонизировать cwd
//!   обязательно: на macOS он сам бывает симлинком (`/tmp` → `/private/tmp`),
//!   и без этого любой путь внутри выглядел бы «снаружи». Симлинк внутри cwd,
//!   ведущий наружу, после канонизации оказывается снаружи и отбивается — ради
//!   этого замок и нужен.
//! * **Только обычный файл или директория.** `File::open` на FIFO или
//!   устройстве повис бы навсегда, а отмену хода блокирующий вызов не слышит.
//!
//! Порядок проверок: сначала «есть ли путь», потом «внутри ли он». Поэтому
//! несуществующий путь снаружи отвечает «нет такого», а не «снаружи»: модель
//! узнаёт о существовании чужого пути, но не о его содержимом.
//!
//! Слова — дело тулов: модуль возвращает причину отказа ([`ConfineError`]), а
//! текст «что не так и что делать» каждый тул пишет сам, от своего имени.
//!
//! Вызовы блокирующие (`std::fs`): из async-кода — через `spawn_blocking`.
//! Почему замок есть и когда его пересматривать — `docs/decisions.md`.

use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

/// Канонизированная рабочая директория — то, о что проверяются пути.
#[derive(Debug, Clone)]
pub(crate) struct CanonicalCwd(PathBuf);

impl CanonicalCwd {
    /// Канонизировать `cwd`. Ошибка — cwd нет или он недоступен.
    pub(crate) fn new(cwd: &Path) -> io::Result<Self> {
        cwd.canonicalize().map(Self)
    }

    /// Канонизированный путь cwd.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Канонизированный путь cwd во владение.
    pub(crate) fn into_path(self) -> PathBuf {
        self.0
    }

    /// Пропустить `path` через замок: канонизировать, проверить, что он внутри
    /// cwd, и что это обычный файл или директория. Относительный `path` — от
    /// cwd, абсолютный — как есть.
    pub(crate) fn confine(&self, path: &Path) -> Result<Confined, ConfineError> {
        // Склеиваем с cwd сами: `canonicalize` резолвил бы относительный путь
        // от cwd процесса, а не агента.
        let path = match self.0.join(path).canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConfineError::NotFound);
            }
            Err(error) => return Err(ConfineError::Unresolvable(error)),
        };
        if !path.starts_with(&self.0) {
            return Err(ConfineError::Outside);
        }
        let metadata = std::fs::metadata(&path).map_err(ConfineError::Metadata)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(ConfineError::NotRegular);
        }
        Ok(Confined { path, metadata })
    }
}

/// Путь, прошедший замок.
#[derive(Debug)]
pub(crate) struct Confined {
    /// Канонический путь внутри cwd.
    pub(crate) path: PathBuf,
    /// Метаданные канонического пути: обычный файл или директория.
    pub(crate) metadata: Metadata,
}

/// Почему путь не прошёл замок.
#[derive(Debug)]
pub(crate) enum ConfineError {
    /// Пути нет (или это висячий симлинк). Отдельно от прочих причин: модель
    /// должна понять «файла нет», а не «доступ запрещён».
    NotFound,
    /// Канонизация упала иначе: нет прав на компонент, компонент — не
    /// директория и т. п.
    Unresolvable(io::Error),
    /// Канонический путь вне cwd: абсолютный путь, `..` или симлинк наружу.
    Outside,
    /// `metadata` канонического пути не снялась.
    Metadata(io::Error),
    /// Не обычный файл и не директория: FIFO, сокет, устройство.
    NotRegular,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// `parent/work` — cwd; `parent/secret.txt` и `parent/work2/` — снаружи.
    fn sandbox() -> (TempDir, CanonicalCwd) {
        let parent = TempDir::new().unwrap();
        write(parent.path(), "secret.txt", "secret");
        write(parent.path(), "work2/secret.txt", "secret");
        write(parent.path(), "work/a.txt", "a");
        std::fs::create_dir_all(parent.path().join("work/sub")).unwrap();
        let cwd = CanonicalCwd::new(&parent.path().join("work")).unwrap();
        (parent, cwd)
    }

    #[test]
    fn paths_inside_the_cwd_are_canonical_files_or_directories() {
        let (_parent, cwd) = sandbox();
        let file = cwd.path().join("a.txt");
        for path in [
            PathBuf::from("a.txt"),
            PathBuf::from("sub/../a.txt"),
            PathBuf::from("./a.txt"),
            file.clone(),
        ] {
            let confined = cwd.confine(&path).unwrap();
            assert_eq!(confined.path, file, "{}", path.display());
            assert!(confined.metadata.is_file(), "{}", path.display());
        }
        let confined = cwd.confine(Path::new("sub")).unwrap();
        assert!(confined.metadata.is_dir());
        // Сам cwd — тоже внутри.
        assert_eq!(cwd.confine(Path::new(".")).unwrap().path, cwd.path());
    }

    #[test]
    fn paths_leading_outside_are_rejected() {
        let (parent, cwd) = sandbox();
        for path in [
            PathBuf::from(".."),
            PathBuf::from("../secret.txt"),
            PathBuf::from("sub/../../secret.txt"),
            // Соседняя директория с общим префиксом имени — тоже снаружи.
            PathBuf::from("../work2/secret.txt"),
            parent.path().join("secret.txt"),
            PathBuf::from("/"),
        ] {
            let result = cwd.confine(&path);
            assert!(
                matches!(result, Err(ConfineError::Outside)),
                "{}: {result:?}",
                path.display()
            );
        }
    }

    #[test]
    fn missing_paths_are_not_found_rather_than_outside() {
        let (_parent, cwd) = sandbox();
        for path in ["nope.txt", "sub/nope.txt"] {
            let result = cwd.confine(Path::new(path));
            assert!(
                matches!(result, Err(ConfineError::NotFound)),
                "{path}: {result:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved_before_the_check() {
        use std::os::unix::fs::symlink;
        let (parent, cwd) = sandbox();
        let work = cwd.path();
        symlink(parent.path().join("secret.txt"), work.join("out_file")).unwrap();
        symlink(parent.path().join("work2"), work.join("out_dir")).unwrap();
        symlink(work.join("a.txt"), work.join("in_file")).unwrap();
        symlink(work.join("gone.txt"), work.join("dangling")).unwrap();

        for path in ["out_file", "out_dir", "out_dir/secret.txt"] {
            let result = cwd.confine(Path::new(path));
            assert!(
                matches!(result, Err(ConfineError::Outside)),
                "{path}: {result:?}"
            );
        }
        // Симлинк внутри cwd, ведущий внутрь, — законен; путь уже канонический.
        let confined = cwd.confine(Path::new("in_file")).unwrap();
        assert_eq!(confined.path, work.join("a.txt"));
        // Висячий симлинк — «нет такого», как и раньше у `read`.
        assert!(matches!(
            cwd.confine(Path::new("dangling")),
            Err(ConfineError::NotFound)
        ));
    }

    /// Грабля macOS (`/tmp` → `/private/tmp`), воспроизведённая на любом unix:
    /// cwd, заданный через симлинк, не делает свои же файлы «внешними».
    #[cfg(unix)]
    #[test]
    fn cwd_behind_a_symlink_still_contains_its_files() {
        let (parent, _cwd) = sandbox();
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(parent.path().join("work"), &alias).unwrap();

        let cwd = CanonicalCwd::new(&alias).unwrap();
        for path in [PathBuf::from("a.txt"), alias.join("a.txt")] {
            assert!(cwd.confine(&path).is_ok(), "{}", path.display());
        }
        assert!(matches!(
            cwd.confine(&alias.join("../secret.txt")),
            Err(ConfineError::Outside)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_not_a_regular_file() {
        let (_parent, cwd) = sandbox();
        let status = std::process::Command::new("mkfifo")
            .arg(cwd.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            cwd.confine(Path::new("pipe")),
            Err(ConfineError::NotRegular)
        ));
    }

    #[test]
    fn missing_cwd_is_an_error() {
        let parent = TempDir::new().unwrap();
        assert!(CanonicalCwd::new(&parent.path().join("nope")).is_err());
    }
}
