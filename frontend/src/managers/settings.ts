export interface UserSettings {
    confirm_accept: boolean;
    confirm_reject: boolean;
    replacement_filter: 'all' | 'replacement' | 'new';
    pending_per_page: number;
}

export default class SettingsManager {
    public static getDefaultSettings(): UserSettings {
        return {
            confirm_accept: false,
            confirm_reject: false,
            replacement_filter: 'all',
            pending_per_page: 12,
        };
    }

    public static getSettings(): UserSettings {
        const settings = localStorage.getItem('settings');
        const parsedSettings = settings ? JSON.parse(settings) : null;
        if (parsedSettings) {
            const defaultSettings = SettingsManager.getDefaultSettings();

            const validateBool = (value: any, defaultValue: boolean): boolean => {
                return typeof value === 'boolean' ? value : defaultValue;
            }

            const validateNumber = (value: any, defaultValue: number): number => {
                return typeof value === 'number' && value > 0 ? value : defaultValue;
            }

            return {
                confirm_accept: validateBool(parsedSettings.confirm_accept, defaultSettings.confirm_accept),
                confirm_reject: validateBool(parsedSettings.confirm_reject, defaultSettings.confirm_reject),
                replacement_filter: ['all', 'replacement', 'new'].includes(parsedSettings.replacement_filter) ? parsedSettings.replacement_filter : defaultSettings.replacement_filter,
                pending_per_page: validateNumber(parsedSettings.pending_per_page, defaultSettings.pending_per_page),
            };
        } else {
            return SettingsManager.getDefaultSettings();
        }
    }

    public static saveSettings(settings: UserSettings): void {
        localStorage.setItem('settings', JSON.stringify(settings));
    }
}